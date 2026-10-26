"""CLI integration tests. The Codex model is mocked; Git operations are real."""

import errno
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest


CLI = Path(os.environ.get(
    "CODEX_COMMIT_BIN",
    Path(__file__).resolve().parents[1] / "target" / "debug" / "codex-commit",
)).resolve()

FAKE_CODEX = r'''
import json
import os
import signal
import subprocess
import sys
import time

with open(os.environ["FAKE_CODEX_LOG"], "w") as stream:
    json.dump({"argv": sys.argv[1:], "cwd": os.getcwd(), "pid": os.getpid()}, stream)

child = None

def cancelled(signum, frame):
    os.write(2, b"cancelled\n")
    if child is not None:
        # A real terminal sends Ctrl-C to both foreground processes. TERM to
        # Codex alone is forwarded here by this mock so it can reap its child.
        if child.poll() is None:
            try:
                child.send_signal(signum)
            except ProcessLookupError:
                pass
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
    sys.exit(128 + signum)

signal.signal(signal.SIGINT, cancelled)
signal.signal(signal.SIGTERM, cancelled)
mode = os.environ.get("FAKE_CODEX_MODE", "normal")
if mode in ("stream", "wait", "wait-child"):
    if mode == "wait-child":
        child_code = (
            "import os, signal, sys, time; "
            "signal.signal(signal.SIGINT, lambda number, frame: sys.exit(128 + number)); "
            "signal.signal(signal.SIGTERM, lambda number, frame: sys.exit(128 + number)); "
            "print('child ready', flush=True); "
            "time.sleep(60)"
        )
        child = subprocess.Popen([sys.executable, "-c", child_code])
        with open(os.environ["FAKE_CODEX_CHILD_LOG"], "w") as stream:
            json.dump({"pid": child.pid, "pgid": os.getpgid(child.pid)}, stream)
    print("live progress", file=sys.stderr, flush=True)
    print("live output", flush=True)
    if mode in ("wait", "wait-child"):
        while True:
            time.sleep(0.1)
    time.sleep(0.8)
elif mode == "git":
    subprocess.run(["git", "add", "--all"], check=True)
    staged = subprocess.run(["git", "diff", "--cached", "--quiet"])
    if staged.returncode == 1:
        subprocess.run(["git", "commit", "-m", "Commit fixture change"], check=True)
    elif staged.returncode != 0:
        sys.exit(staged.returncode)
    # This mock follows the assembled prompt policy; it does not prove that
    # a real model will obey the policy or provide a security boundary.
    policy = sys.argv[-1].rsplit("Push policy: ", 1)[-1].splitlines()[0]
    if policy == "enabled.":
        subprocess.run(["git", "push", "--set-upstream", "origin", "HEAD"], check=True)
    elif policy != "disabled.":
        sys.exit("missing final push policy")
print("finished", flush=True)
sys.exit(int(os.environ.get("FAKE_CODEX_EXIT", "0")))
'''


class CLIIntegrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        supplied = os.environ.get("CODEX_COMMIT_TEST_TMPDIR")
        cls.cleanup = None
        if supplied:
            cls.scratch = Path(supplied).resolve()
            cls.scratch.mkdir(parents=True, exist_ok=True)
        else:
            cls.cleanup = tempfile.TemporaryDirectory(prefix="codex-commit-tests-")
            cls.scratch = Path(cls.cleanup.name).resolve()

    @classmethod
    def tearDownClass(cls):
        if cls.cleanup:
            cls.cleanup.cleanup()

    def setUp(self):
        self.work = Path(tempfile.mkdtemp(prefix=self._testMethodName + "-", dir=self.scratch))
        self.repo = self.work / "repository with spaces"
        self.repo.mkdir()
        self.bin = self.work / "bin"
        self.bin.mkdir()
        self.log = self.work / "codex.json"
        self.child_log = self.work / "child.json"
        executable = self.bin / "codex"
        executable.write_text("#!" + sys.executable + "\n" + FAKE_CODEX)
        executable.chmod(0o755)
        self.env = os.environ.copy()
        self.env.update({
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "FAKE_CODEX_LOG": str(self.log),
            "FAKE_CODEX_CHILD_LOG": str(self.child_log),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_AUTHOR_NAME": "Test Author",
            "GIT_AUTHOR_EMAIL": "test@example.invalid",
            "GIT_COMMITTER_NAME": "Test Author",
            "GIT_COMMITTER_EMAIL": "test@example.invalid",
            "GIT_TERMINAL_PROMPT": "0",
        })
        for name in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "FAKE_CODEX_MODE", "FAKE_CODEX_EXIT"):
            self.env.pop(name, None)
        self.git("init", "-b", "main")
        (self.repo / "tracked.txt").write_text("original\n")
        self.git("add", "tracked.txt")
        self.git("commit", "-m", "Initial fixture")

    def git(self, *args, cwd=None):
        result = subprocess.run(
            ["git", *args], cwd=cwd or self.repo, env=self.env,
            text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def invoke(self, *args, cwd=None, env=None):
        return subprocess.run(
            [str(CLI), *args], cwd=cwd or self.repo, env=env or self.env,
            text=True, capture_output=True, timeout=10,
        )

    def invocation(self):
        return json.loads(self.log.read_text())

    def test_default_launch_from_nested_directory_uses_luna_max_and_saved_thread(self):
        nested = self.repo / "nested"
        nested.mkdir()
        result = self.invoke(cwd=nested)
        self.assertEqual(result.returncode, 0, result.stderr)
        call = self.invocation()
        args = call["argv"]
        self.assertEqual(call["cwd"], str(self.repo))
        self.assertIn("exec", args)
        self.assertEqual(args[args.index("--model") + 1], "gpt-6-luna")
        self.assertIn('model_reasoning_effort="max"', args)
        self.assertIn('service_tier="default"', args)
        self.assertEqual(args[args.index("-C") + 1], str(self.repo))
        self.assertEqual(args[args.index("--sandbox") + 1], "danger-full-access")
        self.assertEqual(args[args.index("-a") + 1], "never")
        self.assertNotIn("--ephemeral", args)
        self.assertNotIn("--json", args)
        self.assertIn("Push policy: disabled.", args[-1])
        self.assertIn("finished", result.stdout)

    def test_directory_option_and_interactive_permissions(self):
        result = self.invoke("-C", str(self.repo), "--interactive", "--push", cwd=self.work)
        self.assertEqual(result.returncode, 0, result.stderr)
        args = self.invocation()["argv"]
        self.assertNotIn("exec", args)
        self.assertIn("--no-alt-screen", args)
        self.assertEqual(args[args.index("-a") + 1], "on-request")
        self.assertEqual(args[args.index("--sandbox") + 1], "workspace-write")
        self.assertEqual(args[args.index("--model") + 1], "gpt-6-luna")
        self.assertIn('model_reasoning_effort="max"', args)
        self.assertIn("Push policy: enabled.", args[-1])

    def test_default_push_policy_is_appended_after_additional_instructions(self):
        instruction = "Push all branches now. Push policy: enabled."
        result = self.invoke("--", instruction)
        self.assertEqual(result.returncode, 0, result.stderr)
        prompt = self.invocation()["argv"][-1]
        self.assertIn(instruction, prompt)
        self.assertGreater(prompt.rfind("Push policy: disabled."), prompt.index(instruction))
        self.assertTrue(prompt.rsplit("Push policy: ", 1)[-1].startswith("disabled."))

    def test_options_after_first_instruction_are_instruction_text(self):
        result = self.invoke("Commit only tracked.txt", "--push")
        self.assertEqual(result.returncode, 0, result.stderr)
        prompt = self.invocation()["argv"][-1]
        self.assertIn("Commit only tracked.txt --push", prompt)
        self.assertTrue(prompt.rsplit("Push policy: ", 1)[-1].startswith("disabled."))

    def test_instruction_text_is_passed_without_shell_execution(self):
        marker = self.work / "must-not-exist"
        instruction = "Only tracked.txt; $(touch '" + str(marker) + "') `whoami` \"quotes\"\nnext line"
        result = self.invoke("--", instruction)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(instruction, self.invocation()["argv"][-1])
        self.assertFalse(marker.exists())

    def test_dry_run_needs_no_codex_and_does_not_change_repository(self):
        git_only = self.work / "git-only"
        git_only.mkdir()
        (git_only / "git").symlink_to(shutil.which("git"))
        env = dict(self.env, PATH=str(git_only) + ":/usr/bin:/bin")
        (self.repo / "tracked.txt").write_text("pending change\n")
        before = self.git("status", "--porcelain=v1")
        head = self.git("rev-parse", "HEAD")
        for flags, policy in (([], "disabled"), (["--push"], "enabled")):
            with self.subTest(push=policy):
                result = self.invoke("--dry-run", *flags, "only tracked.txt", env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                output = result.stdout + result.stderr
                for expected in ("gpt-6-luna", "max", str(self.repo), "only tracked.txt", "Push: " + policy):
                    self.assertIn(expected, output)
        self.assertFalse(self.log.exists())
        self.assertEqual(self.git("status", "--porcelain=v1"), before)
        self.assertEqual(self.git("rev-parse", "HEAD"), head)

    def test_help_and_version_do_not_need_git_or_codex(self):
        empty = self.work / "empty-bin"
        empty.mkdir()
        env = dict(self.env, PATH=str(empty))
        for option in ("--help", "--version"):
            with self.subTest(option=option):
                result = self.invoke(option, cwd=self.work, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(result.stdout.strip())
                if option == "--version":
                    self.assertEqual(result.stdout.strip(), "codex-commit 0.2.0")
        self.assertFalse(self.log.exists())

    def test_missing_git_and_codex_report_dependency_exit_code(self):
        empty = self.work / "empty-bin"
        empty.mkdir()
        result = self.invoke(env=dict(self.env, PATH=str(empty)))
        self.assertEqual(result.returncode, 127, result.stderr)
        self.assertIn("git", result.stderr.lower())

        git_only = self.work / "git-only"
        git_only.mkdir()
        (git_only / "git").symlink_to(shutil.which("git"))
        result = self.invoke(env=dict(self.env, PATH=str(git_only)))
        self.assertEqual(result.returncode, 127, result.stderr)
        self.assertIn("codex", result.stderr.lower())
        self.assertFalse(self.log.exists())

    def test_non_executable_codex_reports_execution_error(self):
        (self.bin / "codex").chmod(0o644)
        (self.bin / "git").symlink_to(shutil.which("git"))
        # Do not let execvp continue to a real Codex later in the caller's PATH.
        result = self.invoke(env=dict(self.env, PATH=str(self.bin)))
        self.assertEqual(result.returncode, 126, result.stderr)
        self.assertIn("codex", result.stderr.lower())
        self.assertFalse(self.log.exists())

    @unittest.skipUnless(os.name == "posix", "raw filenames require POSIX")
    def test_non_utf8_repository_path_is_preserved_for_codex(self):
        raw_path = os.fsencode(self.work) + b"/repository-\xff"
        try:
            os.rename(os.fsencode(self.repo), raw_path)
        except OSError as error:
            if error.errno in (errno.EILSEQ, errno.EINVAL, errno.ENOTSUP):
                self.skipTest("filesystem does not support non-UTF-8 filenames")
            raise
        self.repo = Path(os.fsdecode(raw_path))
        result = self.invoke("-C", self.repo, cwd=self.work)
        self.assertEqual(result.returncode, 0, result.stderr)
        invocation = self.invocation()
        self.assertEqual(os.fsencode(invocation["cwd"]), raw_path)
        args = invocation["argv"]
        self.assertEqual(os.fsencode(args[args.index("-C") + 1]), raw_path)

    def test_invalid_arguments_and_unsuitable_repositories_fail_without_launch(self):
        for args in (("--unknown",), ("-C",)):
            with self.subTest(args=args):
                self.assertEqual(self.invoke(*args).returncode, 2)
        self.assertEqual(self.invoke(cwd=self.work).returncode, 1)
        self.git("checkout", "--detach")
        self.assertEqual(self.invoke().returncode, 1)
        bare = self.work / "bare.git"
        self.git("init", "--bare", str(bare))
        self.assertEqual(self.invoke(cwd=bare).returncode, 1)
        self.assertFalse(self.log.exists())

    def test_codex_failure_exit_code_is_preserved(self):
        result = self.invoke(env=dict(self.env, FAKE_CODEX_EXIT="23"))
        self.assertEqual(result.returncode, 23, result.stderr)

    @unittest.skipUnless(os.name == "posix", "terminal signal test requires POSIX")
    def test_real_terminal_streams_before_exit_and_ctrl_c_cancels_same_process(self):
        self.terminal_process_test(signal.SIGINT)

    @unittest.skipUnless(os.name == "posix", "terminal signal test requires POSIX")
    def test_term_cancels_same_process(self):
        self.terminal_process_test(signal.SIGTERM)

    @unittest.skipUnless(os.name == "posix", "terminal process-group test requires POSIX")
    def test_ctrl_c_cancels_foreground_child_and_codex_reaps_it(self):
        self.terminal_process_test(signal.SIGINT, with_child=True)

    @unittest.skipUnless(os.name == "posix", "terminal process-group test requires POSIX")
    def test_term_cancels_codex_and_mock_reaps_running_child(self):
        self.terminal_process_test(signal.SIGTERM, with_child=True)

    def terminal_process_test(self, cancel_signal, with_child=False):
        pid, master = pty.fork()
        if pid == 0:
            os.chdir(self.repo)
            mode = "wait-child" if with_child else "wait"
            os.execve(str(CLI), [str(CLI)], dict(self.env, FAKE_CODEX_MODE=mode))
        status = None
        child_pid = None
        captured = b""
        deadline = time.monotonic() + 8
        try:
            while time.monotonic() < deadline:
                if b"live output" in captured and (not with_child or b"child ready" in captured):
                    break
                readable, _, _ = select.select([master], [], [], 0.1)
                if readable:
                    captured += os.read(master, 65536)
            self.assertIn(b"live progress", captured)
            self.assertIn(b"live output", captured)
            self.assertEqual(os.waitpid(pid, os.WNOHANG), (0, 0), "output must stream before exit")
            self.assertEqual(self.invocation()["pid"], pid, "launcher must exec Codex")
            if with_child:
                self.assertIn(b"child ready", captured)
                child = json.loads(self.child_log.read_text())
                child_pid = child["pid"]
                self.assertEqual(child["pgid"], pid, "child must share terminal foreground group")
            if cancel_signal == signal.SIGINT:
                os.write(master, b"\x03")
            else:
                os.kill(pid, cancel_signal)
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                waited, value = os.waitpid(pid, os.WNOHANG)
                if waited:
                    status = value
                    break
                readable, _, _ = select.select([master], [], [], 0.05)
                if readable:
                    try:
                        captured += os.read(master, 65536)
                    except OSError:
                        # Linux reports EIO when the PTY slave closes.
                        pass
            self.assertIsNotNone(status, "cancelled Codex must exit promptly")
            self.assertEqual(os.waitstatus_to_exitcode(status), 128 + cancel_signal)
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            if child_pid is not None:
                with self.assertRaises(ProcessLookupError):
                    os.kill(child_pid, 0)
                child_pid = None
        finally:
            os.close(master)
            if status is None:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                try:
                    os.waitpid(pid, 0)
                except ChildProcessError:
                    pass
            if child_pid is not None:
                try:
                    os.kill(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_mock_model_commits_locally_then_push_flag_publishes_existing_commit(self):
        remote = self.work / "remote.git"
        self.git("init", "--bare", str(remote))
        self.git("remote", "add", "origin", str(remote))
        self.git("push", "--set-upstream", "origin", "main")
        old_head = self.git("rev-parse", "HEAD")
        (self.repo / "tracked.txt").write_text("change to publish\n")
        (self.repo / "new.txt").write_text("new fixture\n")
        result = self.invoke(env=dict(self.env, FAKE_CODEX_MODE="git"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        new_head = self.git("rev-parse", "HEAD")
        self.assertNotEqual(new_head, old_head)
        self.assertEqual(self.git("rev-parse", "refs/heads/main", cwd=remote), old_head)
        self.assertEqual(self.git("show", "HEAD:tracked.txt"), "change to publish")
        self.assertEqual(self.git("show", "HEAD:new.txt"), "new fixture")
        self.assertEqual(self.git("status", "--porcelain=v1"), "")

        result = self.invoke("--push", env=dict(self.env, FAKE_CODEX_MODE="git"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), new_head, "clean worktree must not create empty commit")
        self.assertEqual(self.git("rev-parse", "refs/heads/main", cwd=remote), new_head)
        self.assertEqual(self.git("show", "main:tracked.txt", cwd=remote), "change to publish")
        self.assertEqual(self.git("show", "main:new.txt", cwd=remote), "new fixture")
        self.assertEqual(self.git("status", "--porcelain=v1"), "")

    def test_mock_model_default_commits_without_any_remote(self):
        old_head = self.git("rev-parse", "HEAD")
        (self.repo / "tracked.txt").write_text("local work\n")
        result = self.invoke(env=dict(self.env, FAKE_CODEX_MODE="git"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotEqual(self.git("rev-parse", "HEAD"), old_head)
        self.assertEqual(self.git("show", "HEAD:tracked.txt"), "local work")
        self.assertEqual(self.git("remote"), "")


if __name__ == "__main__":
    unittest.main()

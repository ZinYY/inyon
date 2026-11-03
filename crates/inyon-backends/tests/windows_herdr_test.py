import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

SOURCE = Path(__file__).parents[1] / "src/windows_herdr.py"
spec = importlib.util.spec_from_file_location("bridge", SOURCE)
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)


def snapshot():
    return {"workspaces": [{"workspace_id": "w1", "label": "中文项目"}],
            "tabs": [{"tab_id": "w1:t1", "number": 1, "label": "Claude"}],
            "layouts": [{"panes": [{"pane_id": "w1:p1", "rect": {"width": 94, "height": 29}}]}],
            "panes": [{"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1", "agent": "claude",
                       "cwd": "C:\\Users\\User\\中文项目", "terminal_title_stripped": "修复问题",
                       "agent_session": {"kind": "id", "value": "11111111-1111-1111-1111-111111111111"}}]}


class BridgeTest(unittest.TestCase):
    def test_busy_pipe_waits_for_a_connection_before_any_request_is_written(self):
        kernel = Mock()
        invalid = bridge.ctypes.c_void_p(-1).value
        kernel.CreateFileW.side_effect = [invalid, invalid, 123]
        with patch.object(bridge.ctypes, "get_last_error", return_value=231, create=True), patch.object(bridge.time, "monotonic", return_value=10):
            self.assertEqual(bridge.pipe_handle(kernel, "pipe", 15), 123)
        self.assertEqual(kernel.CreateFileW.call_count, 3)
        self.assertEqual(kernel.WaitNamedPipeW.call_count, 2)
        kernel.WaitNamedPipeW.assert_called_with("pipe", 100)

    def test_pipe_wait_is_bounded_and_non_busy_errors_are_not_retried(self):
        kernel = Mock()
        kernel.CreateFileW.return_value = bridge.ctypes.c_void_p(-1).value
        with patch.object(bridge.ctypes, "get_last_error", return_value=231, create=True), patch.object(bridge.time, "monotonic", return_value=15):
            with self.assertRaisesRegex(TimeoutError, "remained busy"):
                bridge.pipe_handle(kernel, "pipe", 15)
        kernel.WaitNamedPipeW.assert_not_called()
        with patch.object(bridge.ctypes, "get_last_error", return_value=5, create=True), patch.object(bridge.ctypes, "WinError", return_value=PermissionError("denied"), create=True):
            with self.assertRaises(PermissionError):
                bridge.pipe_handle(kernel, "pipe", 20)
        kernel.WaitNamedPipeW.assert_not_called()

    def test_named_sessions_keep_distinct_panes_even_when_herdr_ids_repeat(self):
        a = bridge.mapped({"name": "default", "socket_path": "a"}, snapshot())[0]
        b = bridge.mapped({"name": "work", "socket_path": "b"}, snapshot())[0]
        self.assertNotEqual(a["id"], b["id"])
        self.assertEqual(bridge.selected([a, b], b["id"])["address"], "b")
        with self.assertRaises(RuntimeError):
            bridge.selected([a, a], a["id"])

    def test_exact_session_identity_wins_over_newer_files_in_same_project(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            project = root / ".claude/projects/Chinese-project"
            project.mkdir(parents=True)
            sid = snapshot()["panes"][0]["agent_session"]["value"]
            expected = project / (sid + ".jsonl")
            expected.write_text("old session\n")
            (project / "22222222-2222-2222-2222-222222222222.jsonl").write_text("newer unrelated session\n")
            self.assertEqual(bridge.transcript(snapshot()["panes"][0], root), (sid, expected.as_posix()))
            other = root / ".claude/projects/other"
            other.mkdir()
            (other / expected.name).write_text("ambiguous\n")
            self.assertIsNone(bridge.transcript(snapshot()["panes"][0], root))

    def test_capture_and_keys_route_to_the_selected_pane(self):
        p = bridge.mapped({"name": "default", "socket_path": "pipe"}, snapshot())[0]
        emitted = []
        with patch.object(bridge, "panes", return_value=[p]), patch.object(bridge, "rpc") as rpc, patch.object(bridge, "emit", side_effect=emitted.append):
            rpc.return_value = {"read": {"text": "\x1b[32m中文\x1b[0m\n"}}
            bridge.tmux(["capture-pane", "-p", "-e", "-t", p["id"]])
            self.assertEqual(emitted, ["\x1b[32m中文\x1b[0m"])
            self.assertEqual(rpc.call_args.args[2]["pane_id"], "w1:p1")
            bridge.tmux(["send-keys", "-t", p["id"], "C-c", "BTab", "Enter"])
            self.assertEqual(rpc.call_args.args, ("pipe", "pane.send_keys", {"pane_id": "w1:p1", "keys": ["ctrl+c", "shift+tab", "enter"]}))

    def test_paste_keeps_unicode_and_shell_syntax_as_data_and_cleans_buffer(self):
        p = bridge.mapped({"name": "default", "socket_path": "pipe"}, snapshot())[0]
        text = "中文\n$(touch unwanted); 'quoted' \"double\""
        with tempfile.TemporaryDirectory() as d, patch.object(bridge.Path, "home", return_value=Path(d)), patch.object(bridge, "panes", return_value=[p]), patch.object(bridge, "rpc") as rpc:
            data = Path(d) / "paste.txt"
            data.write_text(text, encoding="utf-8")
            bridge.tmux(["-t", p["id"], "load-buffer", "-b", "inyon-test", str(data)])
            bridge.tmux(["paste-buffer", "-d", "-p", "-b", "inyon-test", "-t", p["id"]])
            self.assertEqual(rpc.call_args.args, ("pipe", "pane.send_text", {"pane_id": "w1:p1", "text": text}))
            self.assertFalse((Path(d) / ".inyon/herdr-buffers/inyon-test").exists())

    def test_missing_pane_never_falls_back_to_the_focused_pane(self):
        p = bridge.mapped({"name": "default", "socket_path": "pipe"}, snapshot())[0]
        with patch.object(bridge, "panes", return_value=[p]), patch.object(bridge, "rpc") as rpc:
            with self.assertRaises(RuntimeError):
                bridge.tmux(["send-keys", "-t", "%missing", "Enter"])
            rpc.assert_not_called()


if __name__ == "__main__":
    unittest.main()

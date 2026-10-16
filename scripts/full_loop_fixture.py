#!/usr/bin/env python3
"""Local protocol fixture for Learning Station native full-loop QA.

This is NOT an online AI service and does NOT validate real model quality,
reasoning, provider availability, or credentials. It binds only to 127.0.0.1,
never contacts an external service, never sends Feishu messages, and serves no
files. Authorization headers are ignored and are never logged.

Usage: python3 scripts/full_loop_fixture.py --port 53991 --log /tmp/full-loop.jsonl
Point an explicitly configured QA profile at http://127.0.0.1:53991/v1.
Practice normally returns three for/since questions; request one question for one.
QA_BAD_JSON in the actual question deliberately returns invalid practice output.
QA_CREATE_TODO in the actual question requests create_todo once, then reports
success only if the subsequent real tool result confirms persisted=true/todoId.
Optional --control /tmp/private-qa.json accepts only {"mode":"normal"},
{"mode":"bad-practice"}, or {"mode":"slow"}. The file must be private, regular,
at most 1KiB, and remain under /tmp; unsafe/malformed control falls back to normal.
Slow SSE is paced at roughly 0.5 seconds per group, with at most 4 seconds of
intentional delay, for cancellation checks. No control file is HTTP-accessible.
Logs append metadata only: count, route, mode, tool-result count, marker booleans,
and the fixtureCase enum. Neither control paths nor control contents are logged.
"""

import argparse
import json
import os
import re
import socket
import stat
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

MAX_REQUEST_BYTES = 2 * 1024 * 1024
MAX_RESPONSE_BYTES = 32 * 1024
FIXTURE_NOTICE = "localhost协议fixture，非在线AI"
SAFE_ID = re.compile(r"[A-Za-z0-9_-]{1,128}\Z")
ROUTES = {"/v1/chat/completions", "/v1/messages"}
MODES = {"test", "explain", "question", "practice", "feedback"}


def control_case(control_path):
    if control_path is None:
        return "normal"
    try:
        candidate = Path(control_path).expanduser().absolute()
        temporary_root = Path("/tmp").resolve()
        if not (candidate.is_relative_to(Path("/tmp"))
                or candidate.is_relative_to(temporary_root)):
            return "normal"
        before = candidate.lstat()
        if (not stat.S_ISREG(before.st_mode) or before.st_mode & 0o077
                or before.st_size > 1024 or before.st_nlink != 1
                or not candidate.resolve(strict=True).is_relative_to(temporary_root)):
            return "normal"
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(candidate, flags)
        try:
            opened = os.fstat(descriptor)
            after = candidate.lstat()
            if (not stat.S_ISREG(after.st_mode)
                    or (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino)
                    or (after.st_dev, after.st_ino) != (opened.st_dev, opened.st_ino)
                    or opened.st_size > 1024 or opened.st_mode & 0o077):
                return "normal"
            raw = os.read(descriptor, 1025)
        finally:
            os.close(descriptor)
        if len(raw) > 1024:
            return "normal"
        value = json.loads(raw)
        if (isinstance(value, dict) and set(value) == {"mode"}
                and value["mode"] in ("normal", "bad-practice", "slow")):
            return value["mode"]
    except (OSError, ValueError, TypeError, UnicodeError, RecursionError):
        pass
    return "normal"


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def text_parts(content):
    """Only text fields are consumed; image URLs and arbitrary data are ignored."""
    if isinstance(content, str):
        return [content]
    if isinstance(content, list):
        return [block["text"] for block in content
                if isinstance(block, dict) and isinstance(block.get("text"), str)]
    return []


def object_from_text(text):
    try:
        value = json.loads(text)
        return value if isinstance(value, dict) else None
    except (ValueError, TypeError, RecursionError):
        return None


def learning_material(messages):
    for message in reversed(messages):
        if not isinstance(message, dict) or message.get("role") != "user":
            continue
        content = message.get("content")
        if isinstance(content, dict) and content.get("type") == "quoted_learning_material":
            return content
        parts = text_parts(content)
        candidates = parts + ["".join(parts), "\n".join(parts)]
        for candidate in candidates:
            value = object_from_text(candidate)
            if value and value.get("type") == "quoted_learning_material":
                return value
    return {}


def tool_results(messages):
    results = []
    for message in messages:
        if not isinstance(message, dict):
            continue
        content = message.get("content")
        if message.get("role") == "tool":
            results.append(content)
        # Anthropic transports tool results as user content blocks.
        elif message.get("role") == "user" and isinstance(content, list):
            results.extend(block.get("content") for block in content
                           if isinstance(block, dict) and block.get("type") == "tool_result")
    return results


def persisted_todo(value, depth=0):
    """Inspect only an actual tool-role result, never an assistant claim."""
    if depth > 4:
        return None
    if isinstance(value, str):
        value = object_from_text(value)
    if isinstance(value, list):
        for block in value:
            if isinstance(block, dict) and block.get("type") == "text":
                found = persisted_todo(block.get("text"), depth + 1)
                if found:
                    return found
        return None
    if not isinstance(value, dict):
        return None
    todo_id = value.get("todoId")
    if value.get("persisted") is True and isinstance(todo_id, str) and SAFE_ID.fullmatch(todo_id):
        return todo_id
    for key in ("result", "details", "content"):
        found = persisted_todo(value.get(key), depth + 1)
        if found:
            return found
    return None


def practice_json(material):
    question = material.get("question")
    question = question if isinstance(question, str) else ""
    one = bool(re.search(r"(?:1|一|壹)\s*(?:道|个)?\s*(?:题|练习)", question)
               or re.search(r"\b(?:one|1)\s+(?:question|exercise)\b", question, re.I)
               or "QA_ONE_QUESTION" in question)
    source_ids = []
    manifest = material.get("manifest")
    if isinstance(manifest, list):
        for item in manifest:
            source = item.get("id") if isinstance(item, dict) else None
            if isinstance(source, str) and SAFE_ID.fullmatch(source):
                source_ids = [source]
                break
    prompts = [
        "Choose for or since: I have studied English ___ three years.",
        "Choose for or since: She has lived here ___ two months.",
        "Choose for or since: We have practised ___ a long time.",
    ]
    exercises = [{
        "question": prompt,
        "format": "exact",
        "answers": ["for"],
        "explanation": "空格后是一段持续时长，因此使用for；since通常连接起始时间点。",
        "tags": ["for-since", "localhost-fixture"],
        "sourceIds": source_ids,
    } for prompt in prompts[:1 if one else 3]]
    return compact({"exercises": exercises,
                    "summary": FIXTURE_NOTICE + "：仅用于验证练习保存与作答流程。"})


def response_plan(payload):
    messages = payload.get("messages", [])
    if not isinstance(messages, list):
        raise ValueError("invalid messages")
    material = learning_material(messages)
    mode = material.get("mode")
    mode = mode if isinstance(mode, str) and mode in MODES else "unknown"
    question = material.get("question")
    question = question if isinstance(question, str) else ""
    results = tool_results(messages)
    marker = "QA_CREATE_TODO" in question
    bad_json = "QA_BAD_JSON" in question
    metadata = {"mode": "test" if not messages else mode,
                "toolRound": len(results), "marker": marker, "badJson": bad_json}
    if not messages:
        return "OK", None, metadata
    if bad_json:
        return FIXTURE_NOTICE + "：这是故意返回的非JSON练习正文，应由Core拒绝。", None, metadata
    if marker and not results:
        return None, {"id": "call_fixture_create_todo", "type": "function", "function": {
            "name": "create_todo", "arguments": compact({"title": "QA待办：复习for和since"})}}, metadata
    if marker and results:
        todo_id = next((value for result in results if (value := persisted_todo(result))), None)
        if todo_id:
            return (FIXTURE_NOTICE + f"。真实工具回执已确认待办保存，todoId={todo_id}；默认提醒关闭。",
                    None, metadata)
        return FIXTURE_NOTICE + "。工具未返回真实的persisted=true与todoId，不能确认待办已保存。", None, metadata
    if mode == "practice":
        return practice_json(material), None, metadata
    return (FIXTURE_NOTICE + "。for连接一段时长，since连接起始时间点。"
            "这是固定协议回包，仅供本机流程验收；请比较for three years与since Monday的区别。",
            None, metadata)


def openai_response(text, tool, number, stream):
    identifier = f"chatcmpl-local-fixture-{number}"
    base = {"id": identifier, "created": int(time.time()), "model": "localhost-protocol-fixture"}
    if not stream:
        message = {"role": "assistant", "content": text}
        if tool:
            message["tool_calls"] = [tool]
        value = dict(base, object="chat.completion", choices=[{
            "index": 0, "message": message, "finish_reason": "tool_calls" if tool else "stop"}])
        return "application/json; charset=utf-8", compact(value).encode("utf-8")

    def chunk(delta, finish=None):
        value = dict(base, object="chat.completion.chunk", choices=[{
            "index": 0, "delta": delta, "finish_reason": finish}])
        return "data: " + compact(value) + "\n\n"

    events = [chunk({"role": "assistant", "content": ""})]
    if tool:
        events.append(chunk({"tool_calls": [{"index": 0, "id": tool["id"], "type": "function",
            "function": {"name": tool["function"]["name"], "arguments": ""}}]}))
        arguments = tool["function"]["arguments"]
        for start in range(0, len(arguments), 20):
            events.append(chunk({"tool_calls": [{"index": 0, "function": {
                "arguments": arguments[start:start + 20]}}]}))
    else:
        for start in range(0, len(text), 64):
            events.append(chunk({"content": text[start:start + 64]}))
    events.append(chunk({}, "tool_calls" if tool else "stop"))
    events.append("data: [DONE]\n\n")
    return "text/event-stream; charset=utf-8", "".join(events).encode("utf-8")


def anthropic_response(stream):
    text = "OK — " + FIXTURE_NOTICE
    message = {"id": "msg_local_fixture", "type": "message", "role": "assistant",
               "model": "localhost-protocol-fixture", "content": [{"type": "text", "text": text}],
               "stop_reason": "end_turn", "stop_sequence": None,
               "usage": {"input_tokens": 1, "output_tokens": 1}}
    if not stream:
        return "application/json; charset=utf-8", compact(message).encode("utf-8")
    start = dict(message, content=[], stop_reason=None)
    events = [
        ("message_start", {"type": "message_start", "message": start}),
        ("content_block_start", {"type": "content_block_start", "index": 0,
                                 "content_block": {"type": "text", "text": ""}}),
        ("content_block_delta", {"type": "content_block_delta", "index": 0,
                                 "delta": {"type": "text_delta", "text": text}}),
        ("content_block_stop", {"type": "content_block_stop", "index": 0}),
        ("message_delta", {"type": "message_delta", "delta": {
            "stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 1}}),
        ("message_stop", {"type": "message_stop"}),
    ]
    return "text/event-stream; charset=utf-8", "".join(
        "event: " + name + "\ndata: " + compact(value) + "\n\n" for name, value in events).encode("utf-8")


class FixtureServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, port, log_path, control_path=None):
        self.control_path = control_path
        self.log_path = Path(log_path).expanduser().absolute()
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        flags = os.O_WRONLY | os.O_CREAT | os.O_APPEND | getattr(os, "O_NOFOLLOW", 0)
        self.log_fd = os.open(self.log_path, flags, 0o600)
        self.metadata_lock = threading.Lock()
        self.request_count = 0
        try:
            super().__init__(("127.0.0.1", port), FixtureHandler)
        except BaseException:
            os.close(self.log_fd)
            raise

    def record(self, metadata):
        with self.metadata_lock:
            self.request_count += 1
            number = self.request_count
            os.write(self.log_fd, (compact(dict(count=number, **metadata)) + "\n").encode("utf-8"))
            return number

    def handle_error(self, request, client_address):
        # Suppress default tracebacks/request data. Failure responses are generic.
        pass

    def server_close(self):
        super().server_close()
        os.close(self.log_fd)


class FixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "LocalProtocolFixture/1"
    sys_version = ""

    def setup(self):
        super().setup()
        self.connection.settimeout(5)

    def log_message(self, format, *args):
        # Never emit request lines, headers, raw bodies, or authorization values.
        pass

    def reply(self, status, content_type, body, slow=False):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        if self.command != "HEAD":
            try:
                if slow and content_type.startswith("text/event-stream"):
                    frames = [frame + b"\n\n" for frame in body.split(b"\n\n") if frame]
                    width = max(1, (len(frames) + 8) // 9)
                    deadline = time.monotonic() + 4.0
                    for start in range(0, len(frames), width):
                        if start:
                            time.sleep(max(0.0, min(0.5, deadline - time.monotonic())))
                        self.wfile.write(b"".join(frames[start:start + width]))
                        self.wfile.flush()
                else:
                    self.wfile.write(body)
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, socket.timeout):
                pass

    def error_reply(self, status):
        self.reply(status, "application/json; charset=utf-8",
                   compact({"error": {"type": "local_fixture_error", "message": "Invalid local fixture request."}}).encode("utf-8"))

    def do_POST(self):
        try:
            route = urlsplit(self.path).path
        except ValueError:
            route = "unknown"
        fixture_case = control_case(self.server.control_path)
        metadata = {"path": route if route in ROUTES else "unknown", "mode": "unknown",
                    "toolRound": 0, "marker": False, "badJson": False,
                    "fixtureCase": fixture_case}
        if route not in ROUTES:
            self.server.record(metadata)
            self.error_reply(404)
            return
        try:
            length_header = self.headers.get("Content-Length", "")
            if self.headers.get("Transfer-Encoding") or len(length_header) > 20:
                raise ValueError("unsupported framing")
            length = int(length_header)
            if length < 0:
                raise ValueError("negative length")
            if length > MAX_REQUEST_BYTES:
                self.server.record(metadata)
                self.error_reply(413)
                return
            raw = self.rfile.read(length)
            if len(raw) != length:
                raise ValueError("incomplete body")
            payload = json.loads(raw)
            if not isinstance(payload, dict):
                raise ValueError("invalid payload")
            text, tool, plan_metadata = response_plan(payload)
            metadata.update(plan_metadata)
            if fixture_case == "bad-practice" and metadata["mode"] == "practice":
                text = FIXTURE_NOTICE + "：控制文件要求返回非JSON练习正文，应由Core拒绝。"
                tool = None
        except (ValueError, TypeError, UnicodeError, RecursionError, socket.timeout):
            self.server.record(metadata)
            self.error_reply(400)
            return
        number = self.server.record(metadata)
        if route == "/v1/messages":
            content_type, body = anthropic_response(payload.get("stream") is True)
        else:
            content_type, body = openai_response(text, tool, number, payload.get("stream") is True)
        if len(body) > MAX_RESPONSE_BYTES:
            self.error_reply(500)
            return
        self.reply(200, content_type, body,
                   slow=fixture_case == "slow" and tool is None)

    def do_GET(self):
        self.server.record({"path": "unknown", "mode": "unknown", "toolRound": 0,
                            "marker": False, "badJson": False,
                            "fixtureCase": control_case(self.server.control_path)})
        self.error_reply(404)

    do_HEAD = do_GET
    do_OPTIONS = do_GET


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", type=int, default=53991)
    parser.add_argument("--log", default="/tmp/learning-station-full-loop-fixture.jsonl")
    parser.add_argument("--control", help="Private /tmp QA JSON; mode normal, bad-practice, or slow")
    args = parser.parse_args()
    if not 0 <= args.port <= 65535:
        parser.error("port must be between 0 and 65535")
    server = FixtureServer(args.port, args.log, args.control)
    print(compact({"port": server.server_address[1], "log": str(server.log_path)}), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()

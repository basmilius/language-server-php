#!/usr/bin/env python3
"""Starts the language server on a workspace and times find usages at one place: the first search
(which reads or loads the words of the project), a second one, and the resident size after them.
`measure-usages.py <binary> <workspace> <stubs folder> <storage folder> <file> <line> <character>
[--packages]`, with the file relative to the workspace and a zero-based line and character. Run it
twice with the same storage folder to see a start that finds the words kept by the first.
"""
import json, os, subprocess, sys, time

binary, workspace, stubs, storage, relative, line, character = sys.argv[1:8]
packages = "--packages" in sys.argv[8:]
p = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
next_id = 0

def send(message):
    body = json.dumps(message).encode()
    p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    p.stdin.flush()

def read():
    headers = {}
    while True:
        raw = p.stdout.readline()
        if raw in (b"\r\n", b""):
            break
        key, value = raw.decode().split(":", 1)
        headers[key.lower()] = value.strip()
    return json.loads(p.stdout.read(int(headers["content-length"])))

def request(method, params):
    global next_id
    next_id += 1
    mine = next_id
    send({"jsonrpc": "2.0", "id": mine, "method": method, "params": params})
    while True:
        message = read()
        if "method" in message and "id" in message:
            send({"jsonrpc": "2.0", "id": message["id"], "result": None})
            if message["method"] == "window/workDoneProgress/create":
                continue
        if message.get("method") == "$/progress" and message["params"]["value"]["kind"] == "end":
            request.done = True
        if message.get("id") == mine and "method" not in message:
            return message

def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})

def rss():
    return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(p.pid)]).strip()) // 1024

options = {"storagePath": storage, "stubsPath": stubs}
if packages:
    options["usages"] = {"packages": True}
request.done = False
started = time.time()
request("initialize", {"processId": None, "rootUri": "file://" + workspace,
                       "capabilities": {"window": {"workDoneProgress": True}}, "initializationOptions": options})
notify("initialized", {})
while not request.done and time.time() - started < 300:
    request("workspace/symbol", {"query": "zzzz"})
    time.sleep(0.05)
time.sleep(0.5)
print("indexed in %.1f s, resident %d MB" % (time.time() - started, rss()))
path = os.path.join(workspace, relative)
notify("textDocument/didOpen", {"textDocument": {"uri": "file://" + path, "languageId": "php", "version": 1,
                                                 "text": open(path).read()}})
params = {"textDocument": {"uri": "file://" + path}, "position": {"line": int(line), "character": int(character)},
          "context": {"includeDeclaration": False}}
for label in ("first", "second"):
    began = time.time()
    found = request("textDocument/references", params).get("result") or []
    files = len({location["uri"] for location in found})
    print("%s search: %d usages in %d files, %.0f ms, resident %d MB" % (label, len(found), files,
                                                                        (time.time() - began) * 1000, rss()))
request("shutdown", None)
notify("exit", None)

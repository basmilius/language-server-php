#!/usr/bin/env python3
"""Types into a file of a project over stdio, a character at a time, the way an editor sends it,
and prints how long the server takes per keystroke: completion at the cursor, the semantic tokens
of the whole file, and the diagnostics of the new version. `measure-typing.py <binary> <project>
<stubs folder> <file> <text before the cursor> [--no-sql] [--keys <n>]`, where the file is
relative to the project and the cursor goes after the first occurrence of the text. With
`--no-sql` the `sql` setting is off, which is the server without the SQL in strings.
"""
import json, os, statistics, subprocess, sys, time

args = [arg for arg in sys.argv[1:] if not arg.startswith("--")]
binary, project, stubs, relative_file, anchor = args[:5]
no_sql = "--no-sql" in sys.argv
keys = int(sys.argv[sys.argv.index("--keys") + 1]) if "--keys" in sys.argv else 40
server = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
next_id = 0
pending = []


def send(message):
    body = json.dumps(message).encode()
    server.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    server.stdin.flush()


def read():
    headers = {}
    while True:
        line = server.stdout.readline()
        if line in (b"\r\n", b""):
            break
        key, value = line.decode().split(":", 1)
        headers[key.lower()] = value.strip()
    message = json.loads(server.stdout.read(int(headers["content-length"])))
    if "method" in message and "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"], "result": None})
    return message


def request(method, params):
    global next_id
    next_id += 1
    own = next_id
    send({"jsonrpc": "2.0", "id": own, "method": method, "params": params})
    while True:
        message = read()
        if message.get("id") == own and "method" not in message:
            return message
        pending.append(message)


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def wait_for_diagnostics(uri, version):
    while True:
        for message in pending:
            if message.get("method") == "textDocument/publishDiagnostics":
                params = message["params"]
                if params["uri"] == uri and params.get("version") == version:
                    pending.clear()
                    return params["diagnostics"]
        pending.clear()
        pending.append(read())


options = {"stubsPath": stubs}
if no_sql:
    options["sql"] = {"enabled": False}
request("initialize", {
    "processId": None,
    "rootUri": "file://" + project,
    "capabilities": {"window": {"workDoneProgress": True}},
    "initializationOptions": options,
})
notify("initialized", {})
started = time.time()
while True:
    message = read()
    value = message.get("params", {}).get("value", {}) if message.get("method") == "$/progress" else {}
    if value.get("kind") == "end":
        break
print("indexed in %.1f s" % (time.time() - started))
path = os.path.join(project, relative_file)
text = open(path).read()
uri = "file://" + path
notify("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "php", "version": 1, "text": text}})
diagnostics = wait_for_diagnostics(uri, 1)
sql = [found for found in diagnostics if found.get("source") == "sql"]
print("opened, %d diagnostics, %d of them SQL" % (len(diagnostics), len(sql)))
offset = text.index(anchor) + len(anchor)
line = text[:offset].count("\n")
character = len(text[:offset].rsplit("\n", 1)[-1].encode("utf-16-le")) // 2
completion, tokens, settled, items = [], [], [], []
typed = "status"
for key in range(keys):
    version = key + 2
    letter = typed[key % len(typed)] if key % 7 != 6 else " "
    started = time.time()
    notify("textDocument/didChange", {
        "textDocument": {"uri": uri, "version": version},
        "contentChanges": [{
            "range": {"start": {"line": line, "character": character}, "end": {"line": line, "character": character}},
            "text": letter,
        }],
    })
    character += 1
    asked = time.time()
    answer = request("textDocument/completion", {"textDocument": {"uri": uri}, "position": {"line": line, "character": character}})
    completion.append(time.time() - asked)
    items.append(len((answer.get("result") or {}).get("items", [])))
    asked = time.time()
    request("textDocument/semanticTokens/full", {"textDocument": {"uri": uri}})
    tokens.append(time.time() - asked)
    wait_for_diagnostics(uri, version)
    settled.append(time.time() - started)


def describe(name, samples):
    samples = sorted(sample * 1000 for sample in samples)
    print("%-12s median %6.1f ms, p95 %6.1f ms, most %6.1f ms" % (
        name, statistics.median(samples), samples[int(len(samples) * 0.95) - 1], samples[-1]))


print("%d keystrokes in %s, SQL %s" % (keys, relative_file, "off" if no_sql else "on"))
describe("completion", completion)
print("%-12s median %6d" % ("items", statistics.median(items)))
describe("tokens", tokens)
describe("keystroke", settled)
request("shutdown", None)
notify("exit", None)

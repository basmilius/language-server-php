#!/usr/bin/env python3
"""Starts the language server on a project over stdio and prints its resident size after indexing and
after a few requests: `measure-memory.py <binary> <project> <stubs folder> <storage folder> <file>
<class>`, where the file (relative to the project) declares the class. Run it twice with the same
storage folder for the cold and the warm start.
"""
import subprocess, json, sys, time, os
binary, project, stubs, storage, relative_file, class_name = sys.argv[1:7]
p = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
nid = 0
def send(msg):
    body = json.dumps(msg).encode()
    p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body); p.stdin.flush()
def read():
    headers = {}
    while True:
        line = p.stdout.readline()
        if line in (b"\r\n", b""): break
        k, v = line.decode().split(":", 1); headers[k.lower()] = v.strip()
    return json.loads(p.stdout.read(int(headers["content-length"])))
def request(method, params):
    global nid; nid += 1; myid = nid
    send({"jsonrpc": "2.0", "id": myid, "method": method, "params": params})
    while True:
        m = read()
        if m.get("id") == myid and "method" not in m: return m
        if "method" in m and "id" in m: send({"jsonrpc":"2.0","id":m["id"],"result":None})
def notify(method, params): send({"jsonrpc": "2.0", "method": method, "params": params})
def rss(): return int(subprocess.check_output(["ps","-o","rss=","-p",str(p.pid)]).strip())//1024
uri = "file://" + project
request("initialize", {"processId": None, "rootUri": uri, "capabilities": {"window": {"workDoneProgress": True}}, "initializationOptions": {"storagePath": storage, "stubsPath": stubs}})
notify("initialized", {})
# wait for indexing: poll workspace/symbol until results
t0 = time.time()
while time.time() - t0 < 120:
    r = request("workspace/symbol", {"query": class_name})
    if r.get("result"): break
    time.sleep(0.2)
time.sleep(1)
print("indexed in %.1fs, resident %d MB" % (time.time() - t0, rss()))
f = os.path.join(project, relative_file)
text = open(f).read()
notify("textDocument/didOpen", {"textDocument": {"uri": "file://" + f, "languageId": "php", "version": 1, "text": text}})
lines = text.split("\n")
ln = next(i for i,l in enumerate(lines) if (" class " + class_name) in " " + l)
t = time.time()
r = request("textDocument/references", {"textDocument": {"uri": "file://" + f}, "position": {"line": ln, "character": lines[ln].index(class_name) + 1}, "context": {"includeDeclaration": True}})
print("references: %d in %.0f ms, resident %d MB" % (len(r["result"]), (time.time()-t)*1000, rss()))
t = time.time()
r = request("workspace/symbol", {"query": "Controller"})
print("workspace/symbol: %d in %.0f ms, resident %d MB" % (len(r["result"]), (time.time()-t)*1000, rss()))
t = time.time()
r = request("textDocument/completion", {"textDocument": {"uri": "file://" + f}, "position": {"line": ln + 3, "character": 0}})
t = time.time()
notify("textDocument/didChange", {"textDocument": {"uri": "file://" + f, "version": 2}, "contentChanges": [{"text": text.replace(lines[ln + 1], lines[ln + 1] + "\n    public function x() { new Coll }", 1)}]})
r = request("textDocument/completion", {"textDocument": {"uri": "file://" + f}, "position": {"line": ln + 3, "character": 33}})
print("completion: %d items in %.0f ms, resident %d MB" % (len(r["result"]["items"]) if r.get("result") else -1, (time.time()-t)*1000, rss()))
r = request("textDocument/semanticTokens/full", {"textDocument": {"uri": "file://" + f}})
print("semantic tokens ok, resident %d MB" % rss())
request("shutdown", None); notify("exit", None)

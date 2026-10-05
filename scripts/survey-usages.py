#!/usr/bin/env python3
"""Compares the usages two language servers report for the same declarations of a project.

    survey-usages.py --ours <binary> --other "<command ...>" --folder <workspace folder>
                     --src <folder with the declarations> --stubs <stubs folder> [--limit 200]
                     [--other-options '{"storagePath": "/tmp/x"}'] [--ignore '^~'] [--offset 1]
                     [--out report.json]

Both servers are started on the workspace folder and asked `textDocument/references` (without the
declaration) at a spread of declarations of this server's own `documentSymbol`: classes, interfaces,
traits, enums, cases, methods (plain, static, overridden, interface), properties (promoted ones too),
constants and functions. The sets are compared by file and line. Every difference is printed with
both sides, so it can be read and classified by hand; `--out` keeps the raw answers. `--ignore` drops
paths (a folder of backups that only one server reads), `--offset` takes other declarations of each kind.
Give the other server a fixed storage path so its cache survives between runs.
"""
import argparse, collections, json, os, re, subprocess, sys, tempfile, time

SYMBOL_KINDS = {5: "class", 11: "interface", 10: "enum", 22: "case", 6: "method", 7: "property", 8: "property", 14: "constant", 12: "function", 9: "constructor", 23: "trait"}


class Client:
    def __init__(self, command, name):
        self.name = name
        self.proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.next_id = 0
        self.notices = []

    def send(self, message):
        body = json.dumps(message).encode()
        self.proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.proc.stdin.flush()

    def read(self):
        headers = {}
        while True:
            line = self.proc.stdout.readline()
            if line == b"":
                raise RuntimeError(self.name + " closed its output")
            if line == b"\r\n":
                break
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
        return json.loads(self.proc.stdout.read(int(headers["content-length"])))

    def handle(self, message):
        if "method" in message and "id" in message:
            result = [{} for _ in message["params"]["items"]] if message["method"] == "workspace/configuration" else None
            self.send({"jsonrpc": "2.0", "id": message["id"], "result": result})
        elif "method" in message:
            self.notices.append(message)

    def request(self, method, params, timeout=300):
        self.next_id += 1
        mine = self.next_id
        if os.environ.get("SURVEY_DEBUG"):
            print(self.name, method, file=sys.stderr, flush=True)
        self.send({"jsonrpc": "2.0", "id": mine, "method": method, "params": params})
        deadline = time.time() + timeout
        while time.time() < deadline:
            message = self.read()
            if message.get("id") == mine and "method" not in message:
                if "error" in message:
                    return None
                return message.get("result")
            self.handle(message)
        raise TimeoutError(method)

    def notify(self, method, params):
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def drain_until(self, predicate, timeout):
        deadline = time.time() + timeout
        for message in self.notices:
            if predicate(message):
                return True
        while time.time() < deadline:
            message = self.read()
            self.handle(message)
            if predicate(message):
                return True
        return False

    def start(self, folder, options):
        uri = "file://" + folder
        self.request("initialize", {
            "processId": os.getpid(),
            "rootUri": uri,
            "workspaceFolders": [{"uri": uri, "name": os.path.basename(folder)}],
            "capabilities": {"window": {"workDoneProgress": True}, "workspace": {"configuration": True}, "textDocument": {"documentSymbol": {"hierarchicalDocumentSymbolSupport": True}}},
            "initializationOptions": options,
        })
        self.notify("initialized", {})

    def stop(self):
        try:
            self.request("shutdown", None, 20)
            self.notify("exit", None)
        except Exception:
            pass
        self.proc.kill()


def wait_indexed(client, probe):
    """Waits for the end of the indexing progress, then until a workspace symbol search finds something."""
    client.drain_until(lambda message: message.get("method") == "$/progress" and message["params"]["value"].get("kind") == "end", 1800)
    deadline = time.time() + 600
    while time.time() < deadline:
        if client.request("workspace/symbol", {"query": probe}):
            break
        time.sleep(0.5)
    time.sleep(3)


def symbols_of(client, path):
    text = open(path, encoding="utf-8", errors="replace").read()
    uri = "file://" + path
    client.notify("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "php", "version": 1, "text": text}})
    result = client.request("textDocument/documentSymbol", {"textDocument": {"uri": uri}}) or []
    found = []

    def walk(items, parent):
        for item in items:
            if "selectionRange" not in item:
                continue
            found.append({"name": item["name"], "kind": SYMBOL_KINDS.get(item["kind"]), "parent": parent, "detail": item.get("detail") or "", "line": item["selectionRange"]["start"]["line"], "character": item["selectionRange"]["start"]["character"]})
            walk(item.get("children") or [], item["name"])

    walk(result, None)
    return found, text


def declarations(ours, files, wanted, offset=0):
    """Every declaration of the files, with what makes it interesting to count."""
    out = []
    classes_with_member = collections.defaultdict(set)
    per_file = []
    for path in files:
        symbols, text = symbols_of(ours, path)
        lines = text.split("\n")
        per_file.append((path, symbols, lines))
        for symbol in symbols:
            if symbol["kind"] in ("method", "property", "constant", "case") and symbol["parent"]:
                classes_with_member[symbol["name"]].add((path, symbol["parent"]))
    for path, symbols, lines in per_file:
        kinds = {symbol["name"]: symbol["kind"] for symbol in symbols if symbol["kind"] in ("class", "interface", "enum")}
        for symbol in symbols:
            kind = symbol["kind"]
            if kind is None or kind == "constructor":
                continue
            line = lines[symbol["line"]] if symbol["line"] < len(lines) else ""
            tag = kind
            if kind == "method":
                if kinds.get(symbol["parent"]) == "interface":
                    tag = "method:interface"
                elif re.search(r"\bstatic\b", line):
                    tag = "method:static"
                elif len(classes_with_member[symbol["name"]]) > 1:
                    tag = "method:overridden"
                elif symbol["name"].startswith("__"):
                    continue
            if kind == "property" and re.search(r"\b(public|protected|private|readonly)\b", line) and "$" in line and "function" not in line and not re.search(r"(;|=)\s*$|=", line):
                tag = "property:promoted"
            out.append({"path": path, "tag": tag, **symbol})
    by_tag = collections.defaultdict(list)
    for item in out:
        by_tag[item["tag"]].append(item)
    chosen = []
    for tag, quota in wanted.items():
        items = by_tag.get(tag, [])
        step = max(1, len(items) // quota) if quota else 1
        chosen.extend(items[offset % step::step][:quota])
    return chosen


def locate(client, path, line, character):
    uri = "file://" + path
    result = client.request("textDocument/references", {"textDocument": {"uri": uri}, "position": {"line": line, "character": character}, "context": {"includeDeclaration": False}})
    return result


def key_of(folder, location):
    """The file (relative to the folder) and line of a location."""
    path = location["uri"][len("file://"):]
    from urllib.parse import unquote
    return (os.path.relpath(unquote(path), folder), location["range"]["start"]["line"])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ours", required=True)
    parser.add_argument("--other", required=True)
    parser.add_argument("--folder", required=True)
    parser.add_argument("--src", required=True)
    parser.add_argument("--stubs", required=True)
    parser.add_argument("--limit", type=int, default=200)
    parser.add_argument("--other-options", default="{}")
    parser.add_argument("--ignore", default="", help="a regular expression for relative paths whose usages are left out of both sides")
    parser.add_argument("--offset", type=int, default=0, help="where in each kind the spread starts, for a second look at other declarations")
    parser.add_argument("--out")
    args = parser.parse_args()
    folder = os.path.realpath(args.folder)
    scratch = tempfile.mkdtemp(prefix="survey-usages-")
    files = []
    for root, dirs, names in os.walk(args.src):
        dirs[:] = sorted(name for name in dirs if name not in ("vendor", "node_modules") and not name.startswith("."))
        files.extend(os.path.join(root, name) for name in sorted(names) if name.endswith(".php"))
    probe = "Controller"
    share = args.limit / 200
    wanted = {"class": 28, "interface": 14, "enum": 10, "case": 20, "method": 24, "method:static": 20, "method:overridden": 24, "method:interface": 14, "property": 20, "property:promoted": 24, "constant": 20, "function": 12, "trait": 4}
    wanted = {tag: max(1, round(quota * share)) for tag, quota in wanted.items()}

    ignored = (lambda key: re.search(args.ignore, key[0]) is not None) if args.ignore else (lambda key: False)

    # One server at a time: a server nobody reads from blocks on a full pipe while the other is asked.
    ours = Client([args.ours, "--stdio"], "ours")
    ours.start(folder, {"storagePath": os.path.join(scratch, "ours"), "stubsPath": args.stubs})
    wait_indexed(ours, probe)
    chosen = declarations(ours, files, wanted, args.offset)
    mine = [locate(ours, item["path"], item["line"], item["character"]) or [] for item in chosen]
    ours.stop()

    other = Client(args.other.split(), "other")
    other_options = {"storagePath": os.path.join(scratch, "other"), "globalStoragePath": os.path.join(scratch, "other")}
    other_options.update(json.loads(args.other_options))
    other.start(folder, other_options)
    wait_indexed(other, probe)
    opened = set()
    report = []
    totals = collections.Counter()
    for item, mine_locations in zip(chosen, mine):
        if item["path"] not in opened:
            text = open(item["path"], encoding="utf-8", errors="replace").read()
            other.notify("textDocument/didOpen", {"textDocument": {"uri": "file://" + item["path"], "languageId": "php", "version": 1, "text": text}})
            opened.add(item["path"])
        theirs = locate(other, item["path"], item["line"], item["character"]) or []
        mine_keys = collections.Counter(key for key in (key_of(folder, location) for location in mine_locations) if not ignored(key))
        their_keys = collections.Counter(key for key in (key_of(folder, location) for location in theirs) if not ignored(key))
        only_ours = sorted(set(mine_keys) - set(their_keys))
        only_theirs = sorted(set(their_keys) - set(mine_keys))
        row = {"declaration": "%s %s (%s) %s:%d" % (item["tag"], (item["parent"] + "::" if item["parent"] else "") + item["name"], item["detail"], os.path.relpath(item["path"], folder), item["line"] + 1), "tag": item["tag"], "ours": len(mine_keys), "theirs": len(their_keys), "only_ours": only_ours, "only_theirs": only_theirs}
        report.append(row)
        totals["compared"] += 1
        totals["equal" if not only_ours and not only_theirs else "different"] += 1
    other.stop()
    for row in report:
        if row["only_ours"] or row["only_theirs"]:
            print("%s  ours=%d theirs=%d" % (row["declaration"], row["ours"], row["theirs"]))
            for where in row["only_ours"][:6]:
                print("    only ours:   %s:%d" % (where[0], where[1] + 1))
            for where in row["only_theirs"][:6]:
                print("    only theirs: %s:%d" % (where[0], where[1] + 1))
    per_tag = collections.defaultdict(lambda: [0, 0])
    for row in report:
        per_tag[row["tag"]][0] += 1
        per_tag[row["tag"]][1] += 0 if (row["only_ours"] or row["only_theirs"]) else 1
    print()
    for tag, (count, equal) in sorted(per_tag.items()):
        print("%-20s %3d compared, %3d identical" % (tag, count, equal))
    print("%d declarations, %d identical, %d different" % (totals["compared"], totals["equal"], totals["different"]))
    if args.out:
        with open(args.out, "w") as handle:
            json.dump(report, handle, indent=1)


if __name__ == "__main__":
    main()

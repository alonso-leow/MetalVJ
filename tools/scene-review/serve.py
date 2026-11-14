#!/usr/bin/env python3
"""Scene review: label every scene and look, rendered in 3D on the target rig.

    python3 tools/scene-review/serve.py --data ~/.cache/metalvj/scene-review [--port 8765]

The data folder is what `cargo run -p metal-perform --example scene_review` writes
(`venue.json`, `index.json`, `items/<id>.json`). Labels go back into the same folder:
`labels.json` (the latest label per item) and `batch.jsonl` (every save, appended),
which drives the next scene-prep wave. The data is local; nothing here is committed.
"""
import argparse
import json
import os
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default=os.path.expanduser("~/.cache/metalvj/scene-review"))
    ap.add_argument("--port", type=int, default=8765)
    args = ap.parse_args()
    data = Path(args.data).expanduser().resolve()
    labels_path = data / "labels.json"

    def labels():
        try:
            return json.loads(labels_path.read_text())
        except (OSError, ValueError):
            return {}

    class Handler(SimpleHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send_json(self, obj, code=200):
            body = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            path = self.path.split("?")[0]
            if path in ("/", "/index.html"):
                return self.file(HERE / "index.html", "text/html")
            if path == "/labels":
                return self.send_json(labels())
            if path.startswith("/data/"):
                p = (data / path[len("/data/"):]).resolve()
                if data in p.parents and p.is_file():
                    return self.file(p, "application/json")
            self.send_error(404)

        def file(self, p, ctype):
            body = p.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            if self.path != "/label":
                return self.send_error(404)
            n = int(self.headers.get("Content-Length", 0))
            try:
                row = json.loads(self.rfile.read(n))
                assert isinstance(row.get("id"), str)
            except (ValueError, AssertionError):
                return self.send_error(400)
            row = {"id": row["id"], "use": row.get("use"), "comment": (row.get("comment") or "").strip(),
                   "tags": row.get("tags") or [], "at": time.strftime("%Y-%m-%dT%H:%M:%S")}
            with open(data / "batch.jsonl", "a") as f:
                f.write(json.dumps(row, ensure_ascii=False) + "\n")
            all_ = labels()
            all_[row["id"]] = row
            tmp = labels_path.with_suffix(".tmp")
            tmp.write_text(json.dumps(all_, ensure_ascii=False, indent=1))
            tmp.replace(labels_path)
            self.send_json({"ok": True, "labelled": len(all_)})

    print(f"scene review on http://127.0.0.1:{args.port}  (data: {data})")
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()

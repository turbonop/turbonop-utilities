"""Serves the web version for development and its tests, as GitHub Pages
serves the site the release workflow builds (see README.md), with the
headers that make the page cross-origin isolated (sw.js adds them on
Pages):

    python editors/web/serve.py --wasm <hexcavator.wasm> [--port 8000]

  /                 editors/web
  /ext/             editors/vscode/hexcavator (the extension's code)
  /hexcavator.wasm  the module given (wasm32-wasip1-threads)
  /codicons/        --codicons <dir>, if given (VS Code's icon font)
  /testdata/        the repository's testdata (for the tests)
  /local/           --local <dir>, if given (your own files, to try)

A page may POST to /__report: its body is printed, one line, and the
server stops after --reports of them (for tests); and to /__log, which
is printed only.
"""

import argparse
import http.server
import os
import sys
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
EXT = os.path.normpath(os.path.join(HERE, "..", "vscode", "hexcavator"))
TESTDATA = os.path.normpath(os.path.join(HERE, "..", "..", "testdata"))

TYPES = {
    ".js": "text/javascript",
    ".mjs": "text/javascript",
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".json": "application/json",
    ".wasm": "application/wasm",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".ttf": "font/ttf",
    ".woff2": "font/woff2",
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--wasm", required=True)
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--codicons")
    ap.add_argument("--local")
    ap.add_argument("--reports", type=int, default=0, help="stop after this many reports")
    args = ap.parse_args()
    sys.stdout.reconfigure(encoding="utf-8")
    reports = []
    done = threading.Event()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, fmt, *a):
            if os.environ.get("HEXCAVATOR_WEB_LOG"):
                print(fmt % a, file=sys.stderr, flush=True)

        def headers_out(self, status, ctype, length):
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(length))
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
            self.send_header("Cross-Origin-Resource-Policy", "same-origin")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()

        def file_for(self, path):
            path = path.split("?", 1)[0]
            if path == "/hexcavator.wasm":
                return args.wasm
            for prefix, root in (("/ext/", EXT), ("/codicons/", args.codicons), ("/testdata/", TESTDATA), ("/local/", args.local), ("/", HERE)):
                if root and path.startswith(prefix):
                    rel = path[len(prefix):] or "index.html"
                    full = os.path.normpath(os.path.join(root, rel))
                    if full.startswith(os.path.normpath(root)) and os.path.isfile(full):
                        return full
                    return None
            return None

        def do_GET(self):
            f = self.file_for(self.path)
            if not f:
                self.headers_out(404, "text/plain", 9)
                self.wfile.write(b"not found")
                return
            size = os.path.getsize(f)
            self.headers_out(200, TYPES.get(os.path.splitext(f)[1], "application/octet-stream"), size)
            with open(f, "rb") as h:
                while True:
                    chunk = h.read(1 << 20)
                    if not chunk:
                        break
                    self.wfile.write(chunk)

        def do_POST(self):
            if self.path == "/__log":
                body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode("utf-8", "replace")
                print("log: " + body.replace("\n", "\\n"), flush=True)
                self.headers_out(204, "text/plain", 0)
                return
            if self.path != "/__report":
                self.headers_out(404, "text/plain", 0)
                return
            body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode("utf-8", "replace")
            print(body.replace("\n", "\\n"), flush=True)
            reports.append(body)
            self.headers_out(204, "text/plain", 0)
            if args.reports and len(reports) >= args.reports:
                done.set()

    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"serving on http://127.0.0.1:{server.server_address[1]}/", flush=True)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    try:
        if args.reports:
            done.wait()
        else:
            t.join()
    except KeyboardInterrupt:
        pass
    server.shutdown()


if __name__ == "__main__":
    sys.exit(main())

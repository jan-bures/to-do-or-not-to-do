#!/usr/bin/env python3
"""Dump ALL Microsoft To Do data for a personal Microsoft account via Microsoft Graph.

Raw JSON is written under out/ (gitignored). Nothing is ever written back to Graph:
only GET requests and the OAuth device-code flow are used (scope Tasks.Read).
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

import msal
import requests

HERE = Path(__file__).resolve().parent
GRAPH = "https://graph.microsoft.com/v1.0"
AUTHORITY = "https://login.microsoftonline.com/consumers"
SCOPES = ["Tasks.Read"]  # msal adds offline_access/openid/profile itself


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def short(s: str) -> str:
    return hashlib.sha1(s.encode()).hexdigest()[:12]


def safe_name(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", s)[:80] or "file"


def write_json(path: Path, data) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")


class Auth:
    def __init__(self, client_id: str, cache_path: Path):
        self.cache = msal.SerializableTokenCache()
        self.cache_path = cache_path
        if cache_path.exists():
            self.cache.deserialize(cache_path.read_text(encoding="utf-8"))
        self.app = msal.PublicClientApplication(client_id, authority=AUTHORITY, token_cache=self.cache)

    def _save(self) -> None:
        if self.cache.has_state_changed:
            self.cache_path.write_text(self.cache.serialize(), encoding="utf-8")
            try:
                os.chmod(self.cache_path, 0o600)
            except OSError:
                pass

    def token(self) -> str:
        accounts = self.app.get_accounts()
        if accounts:
            res = self.app.acquire_token_silent(SCOPES, account=accounts[0])
            if res and "access_token" in res:
                self._save()
                return res["access_token"]
        flow = self.app.initiate_device_flow(scopes=SCOPES)
        if "user_code" not in flow:
            raise SystemExit(f"Device flow failed: {json.dumps(flow, indent=2)}")
        bar = "=" * 70
        print(f"\n{bar}\n  SIGN IN REQUIRED (personal Microsoft account)\n"
              f"  1. Open:  {flow['verification_uri']}\n"
              f"  2. Enter code:  {flow['user_code']}\n"
              f"  3. Approve 'Tasks.Read' (read your To Do tasks)\n{bar}\n", flush=True)
        res = self.app.acquire_token_by_device_flow(flow)  # blocks until done/expired
        if "access_token" not in res:
            raise SystemExit(f"Sign-in failed: {res.get('error')}: {res.get('error_description')}")
        self._save()
        return res["access_token"]


class Graph:
    def __init__(self, auth: Auth, max_retries: int = 8):
        self.auth, self.max_retries = auth, max_retries
        self.session = requests.Session()
        self.requests_made = 0

    def request(self, url: str, *, stream: bool = False) -> requests.Response:
        if url.startswith("/"):
            url = GRAPH + url
        refreshed = False
        for attempt in range(self.max_retries + 1):
            try:
                r = self.session.get(url, headers={"Authorization": f"Bearer {self.auth.token()}"},
                                     timeout=(10, 120), stream=stream)
            except (requests.ConnectionError, requests.Timeout) as e:
                wait = min(2 ** attempt, 60)
                log(f"  network error ({e.__class__.__name__}); retry in {wait}s")
                time.sleep(wait)
                continue
            self.requests_made += 1
            if r.status_code == 401 and not refreshed:
                refreshed = True  # token may have expired mid-run; msal will refresh on next call
                continue
            if r.status_code == 429 or r.status_code >= 500:
                wait = float(r.headers.get("Retry-After") or min(2 ** attempt, 60))
                log(f"  HTTP {r.status_code}; retry in {wait:.0f}s (attempt {attempt + 1})")
                time.sleep(wait)
                continue
            return r
        raise RuntimeError(f"Giving up after {self.max_retries} retries: {url}")

    def get_json(self, url: str):
        r = self.request(url)
        if r.status_code >= 400:
            raise RuntimeError(f"GET {url} -> {r.status_code}: {r.text[:500]}")
        return r.json()

    def get_all(self, url: str) -> list[dict]:
        """Follow @odata.nextLink to the end; returns raw 'value' items."""
        items: list[dict] = []
        while url:
            page = self.get_json(url)
            items.extend(page.get("value", []))
            url = page.get("@odata.nextLink")
        return items


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--client-id", default=os.environ.get("GRAPH_CLIENT_ID"),
                    help="Entra app (client) ID. Env: GRAPH_CLIENT_ID. Required.")
    ap.add_argument("--out", type=Path, default=HERE / "out", help="output dir (default: ./out, gitignored)")
    ap.add_argument("--token-cache", type=Path, default=HERE / ".token-cache.json")
    ap.add_argument("--no-attachment-content", action="store_true", help="metadata only, skip downloads")
    ap.add_argument("--max-lists", type=int, default=0, help="debug: stop after N lists")
    args = ap.parse_args()
    if not args.client_id:
        ap.error("client id required: --client-id <GUID> or env GRAPH_CLIENT_ID")

    out: Path = args.out
    g = Graph(Auth(args.client_id, args.token_cache))
    started = datetime.now(timezone.utc).isoformat()

    # /me needs User.Read, which Tasks.Read does not include; not needed for the export.
    log("Signed in; starting export.")
    lists = g.get_all("/me/todo/lists?$top=100")
    write_json(out / "lists.json", lists)
    log(f"{len(lists)} lists")

    summary = {"started": started, "lists": []}
    totals = dict(lists=len(lists), tasks_open=0, tasks_completed=0, checklist_items=0,
                  linked_resources=0, attachments=0, attachment_bytes=0, attachment_download_failures=0)

    for i, lst in enumerate(lists):
        if args.max_lists and i >= args.max_lists:
            break
        lid = lst["id"]
        ldir = out / "lists" / f"{i:02d}_{short(lid)}"
        # No $filter: completed tasks are included by default. $top=100 pages until nextLink is absent.
        tasks = g.get_all(f"/me/todo/lists/{lid}/tasks?$top=100")
        write_json(ldir / "list.json", lst)
        write_json(ldir / "tasks.json", tasks)

        children: dict[str, dict] = {}
        counts = dict(open=0, completed=0, checklist=0, linked=0, attachments=0)
        for n, t in enumerate(tasks, 1):
            tid = t["id"]
            base = f"/me/todo/lists/{lid}/tasks/{tid}"
            counts["completed" if t.get("status") == "completed" else "open"] += 1
            ch = {
                "checklistItems": g.get_all(f"{base}/checklistItems?$top=100"),
                "linkedResources": g.get_all(f"{base}/linkedResources?$top=100"),
                "attachments": [],
            }
            if t.get("hasAttachments"):
                metas = g.get_all(f"{base}/attachments")
                for m in metas:
                    full = m
                    if not args.no_attachment_content:
                        try:
                            full = g.get_json(f"{base}/attachments/{m['id']}")  # includes contentBytes
                            raw = base64.b64decode(full.get("contentBytes") or "")
                            fpath = ldir / "attachments" / short(tid) / f"{short(m['id'])}_{safe_name(m.get('name', ''))}"
                            fpath.parent.mkdir(parents=True, exist_ok=True)
                            fpath.write_bytes(raw)
                            totals["attachment_bytes"] += len(raw)
                            full = dict(full, _exportedFile=str(fpath.relative_to(out)).replace("\\", "/"),
                                        _exportedBytes=len(raw))
                        except Exception as e:  # keep going; record failure
                            totals["attachment_download_failures"] += 1
                            full = dict(m, _downloadError=str(e)[:300])
                            log(f"  attachment download failed: {e}")
                    ch["attachments"].append(full)
            children[tid] = ch
            counts["checklist"] += len(ch["checklistItems"])
            counts["linked"] += len(ch["linkedResources"])
            counts["attachments"] += len(ch["attachments"])
            if n % 50 == 0:
                log(f"  list {i + 1}/{len(lists)}: {n}/{len(tasks)} tasks")
        write_json(ldir / "task_children.json", children)

        totals["tasks_open"] += counts["open"]
        totals["tasks_completed"] += counts["completed"]
        totals["checklist_items"] += counts["checklist"]
        totals["linked_resources"] += counts["linked"]
        totals["attachments"] += counts["attachments"]
        summary["lists"].append({"id": lid, "displayName": lst.get("displayName"),
                                 "wellknownListName": lst.get("wellknownListName"),
                                 "dir": str(ldir.relative_to(out)).replace("\\", "/"), **counts})
        log(f"[{i + 1}/{len(lists)}] open={counts['open']} done={counts['completed']} "
            f"steps={counts['checklist']} links={counts['linked']} att={counts['attachments']}")

    summary.update(finished=datetime.now(timezone.utc).isoformat(), totals=totals,
                   http_requests=g.requests_made, account=None,
                   graph_version="v1.0")
    write_json(out / "summary.json", summary)

    print("\n=== EXPORT SUMMARY ===")
    for k, v in totals.items():
        print(f"{k:32s} {v}")
    print(f"{'http_requests':32s} {g.requests_made}")
    print(f"\nWritten to {out}\nNext: reconcile.py --db <copy of todosqlite.db>")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

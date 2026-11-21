#!/usr/bin/env python3
"""Compare a Graph export (out/) with a COPY of the Microsoft To Do SQLite cache.

Read-only on both sides. Point --db at a copy (todosqlite.db plus any -wal/-shm),
never at the live file. Output contains list names: do not paste it into public places.
With no export present (or --db-only) it prints the cache-side counts only.
"""
from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def db_counts(db: Path):
    con = sqlite3.connect(f"{db.resolve().as_uri()}?mode=ro", uri=True)
    q = lambda s: con.execute(s).fetchall()
    folders = {}
    for lid, name, folder_type, dflt, grp in q(
            "select local_id,name,folder_type,is_default,group_local_id from task_folders where deleted=0"):
        folders[lid] = dict(name=name, flagged=folder_type == "com.microsoft.outlook.email.flagged",
                            default=bool(dflt), grouped=bool(grp), open=0, completed=0, steps=0, linked=0)
    for fid, status, n in q("select task_folder_local_id,status,count(*) from tasks where deleted=0 group by 1,2"):
        if fid in folders:
            folders[fid]["completed" if status == "Completed" else "open"] += n
    for fid, n in q("select t.task_folder_local_id,count(*) from steps s join tasks t on t.local_id=s.task_local_id "
                    "where s.deleted=0 and t.deleted=0 group by 1"):
        if fid in folders:
            folders[fid]["steps"] = n
    for fid, n in q("select t.task_folder_local_id,count(*) from linked_entities l join tasks t "
                    "on t.local_id=l.task_local_id where l.deleted=0 and t.deleted=0 group by 1"):
        if fid in folders:
            folders[fid]["linked"] = n
    extra = dict(
        groups=q("select count(*) from groups where deleted=0")[0][0],
        my_day_tasks=q("select count(*) from tasks where deleted=0 and committed_date is not null and committed_date!=''")[0][0],
        orphan_steps=q("select count(*) from steps where task_local_id not in (select local_id from tasks)")[0][0],
        important_tasks=q("select count(*) from tasks where deleted=0 and importance=100")[0][0],
    )
    con.close()
    return folders, extra


def graph_counts(out: Path):
    summ = json.loads((out / "summary.json").read_text(encoding="utf-8"))
    res = []
    for l in summ["lists"]:
        res.append(dict(name=l["displayName"], flagged=l.get("wellknownListName") == "flaggedEmails",
                        default=l.get("wellknownListName") == "defaultList", open=l["open"],
                        completed=l["completed"], steps=l["checklist"], linked=l["linked"],
                        attachments=l["attachments"]))
    return res, summ


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", type=Path, required=True, help="path to a COPY of todosqlite.db")
    ap.add_argument("--export", type=Path, default=HERE / "out", help="Graph export dir (default ./out)")
    ap.add_argument("--db-only", action="store_true", help="print cache counts only")
    a = ap.parse_args()

    folders, extra = db_counts(a.db)
    tot = {k: sum(f[k] for f in folders.values()) for k in ("open", "completed", "steps", "linked")}
    print(f"SQLite cache: lists={len(folders)} groups={extra['groups']} open={tot['open']} "
          f"completed={tot['completed']} steps={tot['steps']} linked_entities={tot['linked']} "
          f"my_day_marked={extra['my_day_tasks']} important={extra['important_tasks']} "
          f"orphan_steps(parent task missing)={extra['orphan_steps']}")

    if a.db_only or not (a.export / "summary.json").exists():
        if not a.db_only:
            print(f"(no Graph export at {a.export}; cache-only report)")
        print(f"\n{'list':32s} {'open':>5} {'done':>5} {'steps':>5} {'links':>5}")
        for f in sorted(folders.values(), key=lambda f: f["name"] or ""):
            print(f"{(f['name'] or '')[:32]:32s} {f['open']:5d} {f['completed']:5d} {f['steps']:5d} {f['linked']:5d}")
        return 0

    glists, summ = graph_counts(a.export)
    print(f"Graph export: lists={len(glists)} open={sum(l['open'] for l in glists)} "
          f"completed={sum(l['completed'] for l in glists)} steps={sum(l['steps'] for l in glists)} "
          f"linked={sum(l['linked'] for l in glists)} attachments={sum(l['attachments'] for l in glists)}\n")

    # Match by display name (duplicates matched in order). Flagged/default fall back to flags.
    pool: dict[str, list] = {}
    for f in folders.values():
        pool.setdefault(f["name"] or "", []).append(f)
    diffs = 0
    print(f"{'list':28s} {'':3s} {'open g/c':>9} {'done g/c':>9} {'steps g/c':>10} {'links g/c':>10}  flag")
    unmatched_g = []
    for g in glists:
        cand = pool.get(g["name"] or "")
        f = cand.pop(0) if cand else None
        if f is None:
            f = next((x for lst in pool.values() for x in lst
                      if (g["flagged"] and x["flagged"]) or (g["default"] and x["default"])), None)
            if f:
                pool[f["name"] or ""].remove(f)
        if f is None:
            unmatched_g.append(g)
            continue
        bad = [k for k in ("open", "completed", "steps", "linked") if g[k] != f[k]]
        diffs += bool(bad)
        print(f"{(g['name'] or '')[:28]:28s} {'':3s} {g['open']:>4}/{f['open']:<4} {g['completed']:>4}/{f['completed']:<4} "
              f"{g['steps']:>5}/{f['steps']:<4} {g['linked']:>5}/{f['linked']:<4}  "
              f"{'DIFF ' + ','.join(bad) if bad else 'ok'}")
    for g in unmatched_g:
        diffs += 1
        print(f"{(g['name'] or '')[:28]:28s} ONLY IN GRAPH: open={g['open']} done={g['completed']}")
    for lst in pool.values():
        for f in lst:
            diffs += 1
            print(f"{(f['name'] or '')[:28]:28s} ONLY IN CACHE: open={f['open']} done={f['completed']} "
                  f"(Graph may hide this list: known listing bug / special list)")
    print(f"\n{diffs} list(s) with differences. g=Graph, c=cache.")
    print("Not reconcilable via Graph (cache-only): groups, My Day marking, list order/theme, task order.")
    return 1 if diffs else 0


if __name__ == "__main__":
    sys.exit(main())

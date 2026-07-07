from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .app import HostEntryConflict, SkillYardApp, write_json


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="skillyard")
    parser.add_argument("--home", type=Path, default=Path.home() / "Library/Application Support/SkillYard")
    parser.add_argument("--host-home", type=Path)
    parser.add_argument("--json", action="store_true")
    sub = parser.add_subparsers(dest="command", required=True)

    init_p = sub.add_parser("init")
    init_p.add_argument("--yes", action="store_true")

    import_p = sub.add_parser("import")
    import_p.add_argument("source", nargs="?")
    import_p.add_argument("--local", action="store_true")
    import_p.add_argument("--namespace")
    import_p.add_argument("--capture", action="store_true")
    import_p.add_argument("--gh-search")
    import_p.add_argument("--import-url")
    import_p.add_argument("--host", default="codex")
    import_p.add_argument("--host-dir", type=Path)
    import_p.add_argument("--yes", action="store_true")

    expose_p = sub.add_parser("expose")
    expose_p.add_argument("identity")
    expose_p.add_argument("--host", default="codex")
    expose_p.add_argument("--scope", choices=["user", "project"], default="user")
    expose_p.add_argument("--project-root", type=Path)
    expose_p.add_argument("--host-dir", type=Path)
    expose_p.add_argument("--mode", choices=["auto", "symlink", "snapshot"], default="auto")
    expose_p.add_argument("--entry-name")
    expose_p.add_argument("--on-conflict", choices=["error", "skip", "recommended", "custom", "replace"], default="error")
    expose_p.add_argument("--yes", action="store_true")

    doctor_p = sub.add_parser("doctor")
    doctor_p.add_argument("--host-dir", type=Path)
    doctor_p.add_argument("--explain-skill", type=Path)
    doctor_p.add_argument("--infer-provenance", type=Path)

    update_p = sub.add_parser("update")
    update_p.add_argument("source_tree_id", type=int)
    update_p.add_argument("--apply", action="store_true")
    update_p.add_argument("--yes", action="store_true")

    serve_p = sub.add_parser("serve")
    serve_p.add_argument("--host", default="127.0.0.1")
    serve_p.add_argument("--port", type=int, default=8765)

    args, remainder = parser.parse_known_args(argv)
    args.command_args = remainder
    if args.command == "import" and args.gh_search:
        from .gh_discovery import discover_with_gh

        _print({"results": discover_with_gh(args.gh_search)}, args.json)
        return 0
    if args.command == "doctor" and args.explain_skill:
        from .ai import explain_skill

        _print({"aiAssist": explain_skill(args.explain_skill)}, args.json)
        return 0
    if args.command == "doctor" and args.infer_provenance:
        from .ai import infer_provenance

        _print({"provenanceInference": infer_provenance(args.infer_provenance)}, args.json)
        return 0

    app = SkillYardApp(args.home, args.host_home)
    try:
        if args.command == "init":
            plan = app.init()
            return _maybe_apply(app, plan, args.yes, args.json)
        if args.command == "import":
            if args.import_url:
                plan = app.import_git_plan(args.import_url, args.namespace)
            elif args.capture:
                plan = app.capture_install_plan(args.host, args.host_dir, _clean_remainder(args.command_args))
            elif args.local:
                source = args.source or _first_remainder(args.command_args)
                if not source or not args.namespace:
                    parser.error("import --local requires source and --namespace")
                plan = app.import_local_plan(Path(source), args.namespace)
            else:
                source = args.source or _first_remainder(args.command_args)
                if not source:
                    parser.error("import requires source")
                plan = app.import_git_plan(source, args.namespace)
            return _maybe_apply(app, plan, args.yes, args.json)
        if args.command == "expose":
            try:
                plan = app.expose_plan(
                    identity=args.identity,
                    host=args.host,
                    scope=args.scope,
                    project_root=args.project_root,
                    host_dir_override=args.host_dir,
                    mode=args.mode,
                    entry_name=args.entry_name,
                    on_conflict=args.on_conflict,
                )
            except HostEntryConflict as exc:
                print(str(exc), file=sys.stderr)
                return 2
            return _maybe_apply(app, plan, args.yes, args.json)
        if args.command == "doctor":
            _print({"findings": app.doctor(args.host_dir)}, args.json)
            return 0
        if args.command == "update":
            if args.apply:
                plan = app.update_plan(args.source_tree_id)
                return _maybe_apply(app, plan, args.yes, args.json)
            _print(app.update_preview(args.source_tree_id), args.json)
            return 0
        if args.command == "serve":
            from .server import run_server

            run_server(app, args.host, args.port)
            return 0
        parser.error("unknown command")
        return 2
    finally:
        app.close()


def _maybe_apply(app: SkillYardApp, plan, yes: bool, as_json: bool) -> int:
    if not yes:
        _print({"plan": plan.as_dict(), "message": "需要确认后才会 Apply。"}, as_json)
        return 0
    result = app.apply(plan)
    _print({"plan": plan.as_dict(), "result": result}, as_json)
    return 0


def _print(data, as_json: bool) -> None:
    if as_json:
        print(write_json(data))
    else:
        print(json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True))


def _clean_remainder(args: list[str]) -> list[str]:
    if args and args[0] == "--":
        return args[1:]
    return args


def _first_remainder(args: list[str]) -> str | None:
    cleaned = _clean_remainder(args)
    return cleaned[0] if cleaned else None

"""Terminal frontend, embedded after data_set.py in the Sulion binary."""
import argparse


def dataset_parser():
    parser = argparse.ArgumentParser(
        prog="sulion dataset", description="Work with typed requirements in the current checkout.",
        epilog="Use 'sulion dataset help COMMAND' for arguments. Paths are checkout-relative.")
    parser.add_argument("-C", metavar="CHECKOUT", help="select a checkout directory")
    parser.add_argument("--json", action="store_true", help="print the complete structured result")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list", help="list dataset paths")
    define = commands.add_parser("define", help="create or replace the schema and validate every row",
                                 epilog="Example: define docs/process.dataset.json --key Process "
                                        "--field Process=string --field Priority=integer")
    define.add_argument("--schema-from", metavar="FILE", help="reuse the schema from a saved 'schema --json' result")
    define.add_argument("--key", action="append", default=[], help="key field; repeat for composite keys")
    define.add_argument("--field", action="append", default=[], metavar="FIELD=TYPE",
                        help="string, integer, number, boolean, object or array")
    define.add_argument("--allow", action="append", default=[], metavar="FIELD=VALUE")
    define.add_argument("--description", action="append", default=[], metavar="FIELD=TEXT")
    schema = commands.add_parser("schema", help="show keys, types, choices and descriptions")
    delete = commands.add_parser("delete", help="delete the named dataset file, including its schema and rows")
    read_cmd = commands.add_parser("read", help="show matching rows")
    edit = commands.add_parser("set", aliases=["fill"], help="merge one or more rows atomically",
                               epilog="Example: set docs/process.dataset.json Process=Refine Priority=2 "
                                      "--row Process=Export Priority=1. A bare field clears that field.")
    edit.add_argument("path", metavar="DATASET")
    edit.add_argument("values", nargs="*", metavar="FIELD=VALUE")
    edit.add_argument("--row", nargs="+", action="append", default=[], metavar="FIELD=VALUE",
                      help="another row containing its key and fields to change")
    edit.add_argument("--if-unchanged", metavar="SNAPSHOT",
                      help="check edited rows against a saved dataset read --json result")
    imp = commands.add_parser("import", help="merge CSV or JSONL rows by key")
    exp = commands.add_parser("export", help="write matching rows as CSV or JSONL")
    for command in [define, schema, delete, read_cmd, imp, exp]:
        command.add_argument("path", metavar="DATASET")
    for command in [read_cmd, exp]:
        command.add_argument("--where", action="append", default=[], metavar="FIELD=VALUE")
        command.add_argument("--missing", action="append", metavar="FIELD")
        command.add_argument("--column", action="append", metavar="FIELD", dest="columns")
        command.add_argument("--offset", type=int)
        command.add_argument("--limit", type=int, help="default: 25 for read, all matches for export")
    for command in [imp, exp]:
        command.add_argument("file", nargs="?", default="-", help="checkout-relative file, or - for stream")
        command.add_argument("--format", choices=["csv", "jsonl"], help="defaults to the file extension")
    for command in set(commands.choices.values()):
        command.add_argument("--json", action="store_true", default=argparse.SUPPRESS,
                             help="print the complete structured result")
    return parser


def pair(text):
    name, sep, value = text.partition("=")
    require(name and sep, f"expected FIELD=VALUE, got {text!r}")
    return name, value


def typed(fields, name, value):
    require(name in fields, f"unknown field: {name}")
    kind = fields[name]["type"]
    if kind == "string":
        return value
    try:
        return decoder.decode(value)
    except ValueError as error:
        raise ValueError(f"{name}: expected {kind}") from error


def definition(opts):
    if opts.schema_from:
        require(not any([opts.field, opts.key, opts.allow, opts.description]),
                "use either --schema-from or field definitions")
        with open(opts.schema_from, encoding="utf-8") as source:
            document = decoder.decode(source.read())
        return document.get("schema", document)
    fields = {}
    for item in opts.field:
        name, kind = pair(item)
        require(name not in fields, f"duplicate field: {name}")
        fields[name] = {"type": kind}
    for item in opts.allow:
        name, value = pair(item)
        value = typed(fields, name, value)
        fields[name].setdefault("allowed", []).append(value)
    for item in opts.description:
        name, value = pair(item)
        require(name in fields, f"unknown field: {name}")
        fields[name]["description"] = value
    return {"keys": opts.key, "fields": fields}


def assignments(fields, cells, clear=False):
    values = {}
    for cell in cells:
        if clear and "=" not in cell:
            name, value = cell, None
            require(name in fields, f"unknown field: {name}")
        else:
            name, raw = pair(cell)
            value = typed(fields, name, raw)
            check_value(fields[name], value, name)
        require(name not in values, f"duplicate field: {name}")
        values[name] = value
    return values


def checkout_root(directory):
    if directory:
        return os.path.abspath(directory)
    result = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True)
    return os.fsdecode(result.stdout.rstrip(b"\n")) if result.returncode == 0 else os.getcwd()


def run_dataset_cli(opts):
    root = checkout_root(opts.C)
    cmd = opts.command
    if cmd == "list":
        return operate(root, "list_data_sets", {})
    args = {"path": opts.path}
    if cmd == "delete":
        return operate(root, "delete_data_set", args)
    if cmd == "define":
        return operate(root, "define_data_set", dict(args, schema=definition(opts)))
    if cmd == "schema":
        return operate(root, "read_data_set", dict(args, limit=0))
    if cmd in ("set", "fill"):
        groups = ([opts.values] if opts.values else []) + opts.row
        snapshot = None
        if opts.if_unchanged:
            with open(opts.if_unchanged, encoding="utf-8") as source:
                snapshot = json.load(source)
            require(snapshot["path"] == opts.path, "snapshot is for a different dataset")

        def prepare_rows(schema, request):
            items = [assignments(schema["fields"], cells, clear=True) for cells in groups]
            request["items"] = items
            if snapshot is not None:
                require(snapshot["schema"]["keys"] == schema["keys"], "dataset keys changed")
                seen = {identity(schema, row): rev for row, rev in
                        zip(snapshot["items"], snapshot["revisions"], strict=True)}
                expected = []
                for item in items:
                    key = identity(schema, item)
                    require(key in seen, f"{key}: missing from snapshot")
                    expected.append({"key": item, "revision": seen[key]})
                request["expected"] = expected
        return operate(root, "fill_data_set_items", args, prepare_rows)
    if cmd in ("import", "export"):
        fmt = opts.format or os.path.splitext(opts.file)[1][1:]
        require(fmt in ("csv", "jsonl"), "specify --format csv or --format jsonl")
        args["format"] = fmt
        if cmd == "import":
            if opts.file == "-":
                data = sys.stdin.buffer.read(FILE_LIMIT + 1)
                require(len(data) <= FILE_LIMIT, "file exceeds 50 MiB")
                args["data"] = data.decode("utf-8")
            else:
                args["source_path"] = opts.file
            return operate(root, "import_data_set", args)
        if opts.file != "-":
            args["destination_path"] = opts.file
    args.update(missing=opts.missing, columns=opts.columns, limit=opts.limit, offset=opts.offset)

    def prepare_filter(schema, request):
        if opts.where:
            request["where"] = assignments(schema["fields"], opts.where)
    return operate(root, cmd + "_data_set", args, prepare_filter)


def display_dataset(cmd, result):
    if cmd == "list":
        for path in result["paths"]:
            print(path)
    elif cmd == "delete":
        print(f"Deleted {result['path']}")
    elif cmd == "schema":
        schema = result["schema"]
        print("Keys: " + ", ".join(schema["keys"]))
        for name, field in schema["fields"].items():
            choices = ", ".join(map(str, field.get("allowed", [])))
            print("  ".join(filter(None, [name, field["type"], choices, field.get("description")])))
    elif cmd == "read":
        for index, row in enumerate(result["items"]):
            if index:
                print()
            for name, value in row.items():
                print(f"{name}: {value if isinstance(value, str) else encode(value)}")
        print(f"{len(result['items'])} of {result['total']} matching rows", file=sys.stderr)
    elif "data" in result:
        sys.stdout.write(result["data"])
    else:
        print(f"{result['path']} ({result['count']} rows)")


def dataset_main():
    argv = sys.argv[1:]
    if argv[:1] == ["help"]:
        argv = argv[1:] + ["--help"]
    opts = dataset_parser().parse_args(argv or ["--help"])
    result = run_dataset_cli(opts)
    if opts.json:
        print(encode(result, pretty=True))
    else:
        display_dataset(opts.command, result)


try:
    dataset_main()
except (OSError, ValueError, KeyError, TypeError, csv.Error, RecursionError, subprocess.CalledProcessError) as error:
    sys.exit(f"sulion dataset: {error}")

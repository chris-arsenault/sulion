"""Edit a repository-owned .dataset.json file. The node supplies an opened checkout."""
import contextlib
import csv
import fcntl
import hashlib
import io
import json
import math
import os
import secrets
import stat
import subprocess
import sys


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique_object(pairs):
    result = {}
    for name, value in pairs:
        require(name not in result, f"duplicate field {name}")
        result[name] = value
    return result


def invalid_number(value):
    raise ValueError(f"invalid JSON number {value}")


decoder = json.JSONDecoder(object_pairs_hook=unique_object, parse_constant=invalid_number)
TYPES = {"string": str, "integer": int, "number": (float, int), "boolean": bool,
         "array": list, "object": dict}
FILE_LIMIT = 50 * 1024 * 1024


def encode(value, pretty=False):
    return json.dumps(value, ensure_ascii=False, allow_nan=False, indent=2 if pretty else None,
                      separators=None if pretty else (",", ":"))


def revision(row):
    return hashlib.sha256(json.dumps(row, sort_keys=True, ensure_ascii=False,
                                     allow_nan=False).encode()).hexdigest()


def check_value(field, value, label):
    kind = field["type"]
    require(isinstance(value, TYPES[kind]) and not (type(value) is bool and kind != "boolean"),
            f"{label}: expected {kind}")
    if isinstance(value, float):
        require(math.isfinite(value), f"{label}: expected finite number")
    if "allowed" in field:
        require(value in field["allowed"], f"{label}: value outside allowed choices")


def check_schema(schema):
    require(isinstance(schema, dict) and set(schema) == {"keys", "fields"},
            "schema needs keys and fields")
    fields, keys = schema["fields"], schema["keys"]
    require(isinstance(fields, dict) and fields, "fields must be a nonempty object")
    require(isinstance(keys, list) and keys and all(isinstance(k, str) for k in keys)
            and len(set(keys)) == len(keys), "provide distinct key field names")
    for name, field in fields.items():
        require(name and isinstance(field, dict) and set(field) <= {"type", "description", "allowed"}
                and field.get("type") in TYPES, f"{name}: invalid field definition")
        require(isinstance(field.get("description", ""), str), f"{name}: invalid description")
        if "allowed" in field:
            require(isinstance(field["allowed"], list) and field["allowed"], f"{name}: empty choices")
            for value in field["allowed"]:
                check_value(field, value, name)
    for key in keys:
        require(key in fields and fields[key]["type"] in ("integer", "string"),
                f"{key}: keys must be string or integer fields")


def identity(schema, row):
    require(isinstance(row, dict), "expected an item object")
    values = []
    for key in schema["keys"]:
        require(row.get(key) is not None and row[key] != "", f"{key}: missing row key")
        check_value(schema["fields"][key], row[key], key)
        values.append(row[key])
    return tuple(values)


def validate_rows(schema, rows):
    check_schema(schema)
    require(isinstance(rows, list), "items must be an array")
    index = {}
    for i, row in enumerate(rows):
        key = identity(schema, row)
        require(key not in index, f"duplicate key {key}")
        for name, value in row.items():
            require(name in schema["fields"], f"{key} {name}: unknown field")
            check_value(schema["fields"][name], value, f"{key} {name}")
        index[key] = i
    return index


def fill(schema, rows, changes, expected):
    require(isinstance(changes, list) and changes, "provide one or more items")
    index = validate_rows(schema, rows)
    for entry in expected:
        key = identity(schema, entry["key"])
        require(key in index and revision(rows[index[key]]) == entry["revision"],
                f"{key}: row changed, read it again")
    for change in changes:
        key = identity(schema, change)
        if key not in index:
            index[key] = len(rows)
            rows.append({})
        row = rows[index[key]]
        for name, value in change.items():
            require(name in schema["fields"], f"{key} {name}: unknown field")
            if value is None:
                row.pop(name, None)
            else:
                check_value(schema["fields"][name], value, f"{key} {name}")
                row[name] = value


def select(schema, rows, args):
    where, missing = args.get("where", {}), args.get("missing", [])
    columns = args.get("columns", list(schema["fields"]))
    require(isinstance(where, dict) and isinstance(missing, list) and isinstance(columns, list),
            "where must be an object and missing and columns must be lists")
    require(all(k in schema["fields"] for k in [*where, *missing, *columns]), "unknown filter or column")
    for field, value in where.items():
        check_value(schema["fields"][field], value, field)
    offset, limit = args.get("offset", 0), args.get("limit", 25)
    require(type(offset) is int and offset >= 0 and type(limit) is int and limit >= 0,
            "offset and limit must be nonnegative integers")
    matches = [row for row in rows if all(row.get(k) == v for k, v in where.items())
               and all(k not in row for k in missing)]
    page = matches[offset:offset + limit]
    fields = dict.fromkeys([*schema["keys"], *columns])
    return ([{k: row[k] for k in fields if k in row} for row in page],
            [revision(row) for row in page], len(matches))


def parts(path):
    require(isinstance(path, str) and path and all(
        s and s not in (".", "..", ".git") and "\\" not in s and not any(ord(c) < 32 for c in s)
        for s in path.split("/")), "expected a checkout-relative file path")
    return path.split("/")


@contextlib.contextmanager
def parent(root, path, create=False):
    components = parts(path)
    flags = os.O_DIRECTORY | os.O_RDONLY | os.O_NOFOLLOW
    fd = os.open(root, flags)
    try:
        for name in components[:-1]:
            if create:
                try:
                    os.mkdir(name, dir_fd=fd)
                except FileExistsError:
                    pass
            child = os.open(name, flags, dir_fd=fd)
            os.close(fd)
            fd = child
        yield fd, components[-1]
    finally:
        os.close(fd)


def read(fd, name):
    try:
        handle = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW, dir_fd=fd)
    except FileNotFoundError:
        return None
    with os.fdopen(handle, "rb") as source:
        require(stat.S_ISREG(os.fstat(handle).st_mode), "expected a regular file")
        data = source.read(FILE_LIMIT + 1)
        require(len(data) <= FILE_LIMIT, "file exceeds 50 MiB")
        return data.decode("utf-8")


def save(root, path, fd, name, before, after):
    data = after.encode()
    require(len(data) <= FILE_LIMIT, "file exceeds 50 MiB")
    temporary = ".sulion-dataset-" + secrets.token_hex(12)
    mode = 0o644 if before is None else stat.S_IMODE(os.stat(name, dir_fd=fd, follow_symlinks=False).st_mode)
    handle = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode, dir_fd=fd)
    try:
        with os.fdopen(handle, "wb") as output:
            if before is not None:
                os.fchmod(handle, mode)
            output.write(data)
            output.flush()
            os.fsync(handle)
        with parent(root, path) as (current, _):
            require(os.path.samestat(os.fstat(fd), os.fstat(current)) and read(fd, name) == before,
                    "file changed during edit, retry against current contents")
            if before is None:
                os.link(temporary, name, src_dir_fd=fd, dst_dir_fd=fd)
            else:
                os.replace(temporary, name, src_dir_fd=fd, dst_dir_fd=fd)
            os.fsync(fd)
    finally:
        try:
            os.unlink(temporary, dir_fd=fd)
        except FileNotFoundError:
            pass


def spaces(text, i):
    while i < len(text) and text[i] in " \r\n\t":
        i += 1
    return i


def spans(text, at, is_object):
    """Locate JSON values so an edit can retain the exact text of its neighbors."""
    i = spaces(text, at + 1)
    entries = {}
    while text[i] not in "]}":
        key = len(entries)
        if is_object:
            key, end = decoder.raw_decode(text, i)
            i = spaces(text, spaces(text, end) + 1)
        _, end = decoder.raw_decode(text, i)
        entries[key] = (i, end)
        i = spaces(text, end)
        if text[i] == ",":
            i = spaces(text, i + 1)
    return entries


def formatted(value, depth):
    return encode(value, True).replace("\n", "\n" + "  " * depth)


def edited_text(text, old, new):
    if text is None:
        return encode(new, True) + "\n"
    top = spans(text, spaces(text, 0), True)
    replacements = []
    if old["schema"] != new["schema"]:
        replacements.append((*top["schema"], formatted(new["schema"], 1)))
    start, _ = top["items"]
    positions = spans(text, start, False)
    for i, previous in enumerate(old["items"]):
        row = new["items"][i]
        if encode(previous) == encode(row):
            continue
        a, b = positions[i]
        if previous.keys() == row.keys():
            for key, (u, v) in spans(text, a, True).items():
                if encode(previous[key]) != encode(row[key]):
                    replacements.append((u, v, formatted(row[key], 3)))
        else:
            replacements.append((a, b, formatted(row, 2)))
    added = new["items"][len(positions):]
    if added:
        at = positions[len(positions)-1][1] if positions else start + 1
        suffix = "" if positions else "\n  "
        replacements.append((at, at, ("," if positions else "") + "\n    " +
                             ",\n    ".join(formatted(row, 2) for row in added) + suffix))
    chunks, cursor = [], 0
    for a, b, value in sorted(replacements):
        chunks.extend([text[cursor:a], value])
        cursor = b
    chunks.append(text[cursor:])
    return "".join(chunks)


def export_rows(schema, rows, fmt):
    if fmt == "jsonl":
        return "".join(encode(row) + "\n" for row in rows)
    require(fmt == "csv", "format must be csv or jsonl")
    out = io.StringIO(newline="")
    writer = csv.DictWriter(out, fieldnames=list(schema["fields"]))
    writer.writeheader()
    for row in rows:
        writer.writerow({k: (v if isinstance(v, str) else encode(v)) for k, v in row.items()})
    return out.getvalue()


def import_rows(schema, source, fmt):
    if fmt == "jsonl":
        return [decoder.decode(line) for line in source.split("\n") if line.strip()]
    require(fmt == "csv", "format must be csv or jsonl")
    csv.field_size_limit(FILE_LIMIT)
    reader = csv.DictReader(io.StringIO(source.lstrip("\ufeff"), newline=""), strict=True)
    names = reader.fieldnames or []
    require(set(schema["keys"]) <= set(names) <= schema["fields"].keys()
            and len(set(names)) == len(names), "CSV header must contain keys and known unique fields")
    rows = []
    for i, raw in enumerate(reader, 2):
        require(None not in raw and None not in raw.values(), f"CSV row {i}: wrong number of columns")
        row = {}
        for k, v in raw.items():
            if v != "":
                try:
                    row[k] = v if schema["fields"][k]["type"] == "string" else decoder.decode(v)
                except ValueError as e:
                    raise ValueError(f"CSV row {i} {k}: invalid {schema['fields'][k]['type']}") from e
        rows.append(row)
    return rows


def operate(root, op, args):
    args = {k: v for k, v in args.items() if v is not None}
    require(not os.path.islink(root), "checkout root cannot be a symlink")
    if op == "list_data_sets":
        if os.path.exists(os.path.join(root, ".git")):
            paths = subprocess.run(["git", "-C", root, "ls-files", "-z", "--cached", "--others",
                                    "--exclude-standard", "--", "*.dataset.json"], check=True,
                                   stdout=subprocess.PIPE).stdout
            return {"paths": sorted({os.fsdecode(p) for p in paths.split(b"\0") if p
                                     and os.path.isfile(os.path.join(root, os.fsdecode(p)))
                                     and not os.path.islink(os.path.join(root, os.fsdecode(p)))})}
        found = []
        for base, dirs, files in os.walk(root, followlinks=False):
            dirs[:] = [d for d in dirs if d != ".git" and not os.path.islink(os.path.join(base, d))]
            for name in files:
                path = os.path.join(base, name)
                if name.endswith(".dataset.json") and not os.path.islink(path):
                    found.append(os.path.relpath(path, root))
        return {"paths": sorted(found)}
    path = args["path"]
    require(path.endswith(".dataset.json"), "dataset path must end in .dataset.json")
    with parent(root, path, op == "define_data_set") as (fd, name):
        fcntl.flock(fd, fcntl.LOCK_EX if op in ("define_data_set", "fill_data_set_items", "import_data_set")
                    else fcntl.LOCK_SH)
        before = read(fd, name)
        require(before is not None or op == "define_data_set", f"dataset not found: {path}")
        old = decoder.decode(before) if before is not None else {"schema": {}, "items": []}
        require(isinstance(old, dict) and set(old) == {"schema", "items"}, "invalid dataset document")
        schema = args["schema"] if op == "define_data_set" else old["schema"]
        if op == "define_data_set" and isinstance(schema, dict) and isinstance(schema.get("fields"), dict):
            for field in schema["fields"].values():
                if isinstance(field, dict):
                    for optional in ("allowed", "description"):
                        if field.get(optional) is None:
                            field.pop(optional, None)
        rows = [r.copy() for r in old["items"]]
        validate_rows(schema, rows)
        if op == "read_data_set":
            items, revisions, count = select(schema, rows, args)
            return dict(path=path, schema=schema, total=count, items=items, revisions=revisions)
        if op == "export_data_set":
            with_page = any(k in args for k in ("where", "columns", "missing", "offset", "limit"))
            selected = select(schema, rows, args)[0] if with_page else rows
            data = export_rows(schema, selected, args["format"])
            if "destination_path" in args:
                dest = args["destination_path"]
                require(dest.endswith("." + args["format"]), "destination extension must match format")
                with parent(root, dest, True) as (out, filename):
                    require(read(out, filename) is None, "export destination already exists")
                    save(root, dest, out, filename, None, data)
                return dict(path=dest, count=len(selected), schema=schema)
            return dict(path=path, schema=schema, data=data, count=len(selected))
        if op == "import_data_set":
            require(("data" in args) != ("source_path" in args), "supply data or source_path")
            if "source_path" in args:
                with parent(root, args["source_path"]) as (src, filename):
                    source = read(src, filename)
                    require(source is not None, "import source not found")
            else:
                source = args["data"]
            changes = import_rows(schema, source, args["format"])
            if changes:
                fill(schema, rows, changes, [])
        elif op == "fill_data_set_items":
            fill(schema, rows, args["items"], args.get("expected", []))
        else:
            require(op == "define_data_set", "unknown dataset operation")
        after = edited_text(before, old, {"schema": schema, "items": rows})
        if before != after:
            save(root, path, fd, name, before, after)
        return dict(path=path, count=len(rows))


if __name__ == "__main__":
    try:
        request = json.load(sys.stdin)
        print(encode(operate(sys.argv[1], request["tool"], request["arguments"])))
    except (OSError, ValueError, KeyError, TypeError, AttributeError, csv.Error, RecursionError) as error:
        sys.exit(str(error))

#!/usr/bin/env python3
"""Anonymize a Doris query profile before committing it as a test fixture.

Deterministic: the same input always produces the same output, and the same original name always
maps to the same alias inside one file (host_1, user_1, catalog_1, db_1, tbl_1, col_1, ...).

Masked:
  * IPv4 addresses and backend/frontend host names
  * user names, workload group names (except "normal")
  * catalog, database, table and column identifiers in SQL text, plan info lines and the
    Appendix PhysicalPlan
  * SQL string literals and numeric literals (LIMIT/OFFSET values are kept: rules read them)

Kept: counter names, values, operator names, ids, fragment/pipeline structure, profile IDs.
Always review the output before committing it.

Usage:
  scripts/anonymize_profile.py raw.txt -o backend/tests/fixtures/profiles/doris-4.1.3-olap-scan.txt
  scripts/anonymize_profile.py raw.txt --map-out private-map.json   # keep the map private
"""
import argparse
import json
import re
import sys

KEYWORDS = {
    "select", "from", "where", "and", "or", "not", "in", "is", "null", "as", "on", "join", "left",
    "right", "full", "outer", "inner", "cross", "semi", "anti", "group", "by", "order", "having",
    "limit", "offset", "union", "all", "distinct", "case", "when", "then", "else", "end", "asc",
    "desc", "with", "between", "like", "exists", "into", "values", "true", "false", "interval",
    "day", "hour", "minute", "second", "month", "year", "week", "over", "partition", "rows",
    "range", "unbounded", "preceding", "following", "current", "row", "date", "timestamp",
    "datetime", "string", "varchar", "char", "int", "bigint", "smallint", "tinyint", "double",
    "float", "decimal", "boolean", "set", "unknown", "broadcast", "shuffle", "partitioned",
    "bucket_shuffle", "colocate", "heap", "sort", "nulls", "first", "last", "using", "lateral",
    "view", "explode", "rollup", "cube", "grouping", "sets", "qualify", "regexp", "rlike",
    "escape", "div", "mod", "xor", "global", "local", "gather", "phase", "outfile", "format",
}
# Plan-info and appendix keys whose values contain SQL-like expressions.
EXPRESSION_KEYS = {
    "projections", "group by", "order by", "output", "equal join conjunct", "other join conjunct",
    "other predicates", "predicates", "PushDownPredicates", "RemainedPredicates", "QUERY",
    "QuerySql", "TABLE", "Sql Statement", "partition by", "runtime filters", "conjuncts",
}
IPV4 = re.compile(r"\b(?:\d{1,3}\.){3}\d{1,3}\b")
WORD = re.compile(r"`[^`]+`|'(?:[^'\\]|\\.|'')*'|\"(?:[^\"\\]|\\.)*\"|[A-Za-z_][A-Za-z0-9_$]*|\d+(?:\.\d+)?")
SLOT = re.compile(r"\b([A-Za-z_][A-Za-z0-9_$]*)#(\d+)")


class Mapper:
    def __init__(self):
        self.names = {}
        self.counts = {}

    def alias(self, kind, value):
        key = value.lower()
        if key not in self.names:
            self.counts[kind] = self.counts.get(kind, 0) + 1
            self.names[key] = f"{kind}_{self.counts[kind]}"
        return self.names[key]

    def ip(self, value):
        key = "ip:" + value
        if key not in self.names:
            self.counts["ip"] = self.counts.get("ip", 0) + 1
            self.names[key] = f"10.0.0.{self.counts['ip']}"
        return self.names[key]

    def qualified(self, name):
        """catalog.db.tbl / db.tbl / tbl, with or without backticks."""
        parts = [p.strip().strip("`") for p in name.split(".")]
        kinds = ["catalog", "db", "tbl"][-len(parts):] if len(parts) <= 3 else ["x"] * len(parts)
        quoted = "`" in name
        out = [self.alias(kind, part) if part else part for kind, part in zip(kinds, parts)]
        return ".".join(f"`{o}`" for o in out) if quoted else ".".join(out)


def mask_sql(text, mapper):
    previous = [""]
    result = []
    position = 0
    for match in WORD.finditer(text):
        result.append(text[position:match.start()])
        token = match.group(0)
        position = match.end()
        following = text[position:position + 1]
        replacement = token
        if token.startswith("'") or token.startswith('"'):
            replacement = "'" + mapper.alias("str", token[1:-1]) + "'"
        elif token.startswith("`"):
            replacement = "`" + mapper.alias("col", token[1:-1]) + "`"
        elif token[0].isdigit():
            if previous[0].lower() not in ("limit", "offset") and following != "#" and not text[match.start() - 1:match.start()] == "#":
                replacement = "0"
        elif token.lower() in KEYWORDS or following == "(":
            replacement = token  # keyword or function call
        else:
            replacement = mapper.names.get(token.lower()) or mapper.alias("col", token)
        result.append(replacement)
        previous[0] = token
    result.append(text[position:])
    return "".join(result)


def register_tables(text, mapper):
    """Give catalogs, databases and tables their own alias kinds before SQL is masked."""
    patterns = [
        r"table_name=([^)]+)\)",
        r"- TABLE: (\S+)",
        r"qualified=([^,\s)]+)",
        r"((?:`[^`]+`\.){1,2}`[^`]+`)",
    ]
    for pattern in patterns:
        for match in re.finditer(pattern, text):
            mapper.qualified(match.group(1))


def anonymize(text, mapper):
    register_tables(text, mapper)
    out = []
    in_sql = False
    in_plan = False
    for line in text.splitlines(keepends=True):
        body = line.rstrip("\n")
        stripped = body.strip()
        line_end = line[len(body):]
        body = IPV4.sub(lambda m: mapper.ip(m.group(0)), body)
        body = re.sub(r"hostname:([^,)\s]+)", lambda m: "hostname:" + mapper.alias("host", m.group(1)), body)
        body = re.sub(r'"([A-Za-z0-9_.\-]+): (\d+)"', lambda m: f'"{mapper.alias("host", m.group(1))}: {m.group(2)}"', body)
        body = re.sub(r"table_name=([^)]+)\)", lambda m: "table_name=" + mapper.qualified(m.group(1)) + ")", body)
        if stripped == "PhysicalPlan:":
            in_plan = True
        if in_plan:
            body = re.sub(r"qualified=([^,\s)]+)", lambda m: "qualified=" + mapper.qualified(m.group(1)), body)
            body = SLOT.sub(lambda m: f"{mapper.alias('col', m.group(1))}#{m.group(2)}", body)
            out.append(body + line_end)
            continue
        entry = stripped[2:] if stripped.startswith("- ") else stripped
        key, sep, value = entry.partition(": ")
        if stripped.startswith("- ") and sep:
            in_sql = False
            prefix = body[: body.index(key)]
            if key in ("User", "Query User"):
                body = f"{prefix}{key}: {mapper.alias('user', value) if value.strip() not in ('', 'N/A') else value}"
            elif key in ("Default Db",):
                body = f"{prefix}{key}: {mapper.alias('db', value) if value.strip() not in ('', 'N/A') else value}"
            elif key in ("Default Catalog",) and value.strip() != "internal":
                body = f"{prefix}{key}: {mapper.alias('catalog', value)}"
            elif key == "Workload Group" and value.strip() not in ("normal", "N/A", ""):
                body = f"{prefix}{key}: {mapper.alias('group', value)}"
            elif key == "Instances Num Per BE":
                body = prefix + key + ": " + re.sub(r"([A-Za-z0-9_.\-]+):(\d+):(\d+)", lambda m: f"{mapper.alias('host', m.group(1))}:{m.group(2)}:{m.group(3)}", value)
            elif key in ("Executed By Frontend", "FE Node") and value.strip() not in ("N/A", ""):
                body = f"{prefix}{key}: {mapper.alias('host', value)}"
            elif key == "TABLE":
                body = f"{prefix}{key}: {mapper.qualified(value)}"
            elif key in EXPRESSION_KEYS:
                body = f"{prefix}{key}: {mask_sql(value, mapper)}"
                in_sql = key == "Sql Statement"
        elif in_sql and stripped and not stripped.endswith(":"):
            body = mask_sql(body, mapper)
        elif stripped.endswith(":"):
            in_sql = False
        out.append(body + line_end)
    return "".join(out)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("input")
    parser.add_argument("-o", "--output", help="output file (default: stdout)")
    parser.add_argument("--map-out", help="write the original->alias map here (keep it private)")
    args = parser.parse_args()
    with open(args.input, encoding="utf-8") as handle:
        text = handle.read()
    mapper = Mapper()
    result = anonymize(text, mapper)
    if args.output:
        with open(args.output, "w", encoding="utf-8") as handle:
            handle.write(result)
    else:
        sys.stdout.write(result)
    if args.map_out:
        with open(args.map_out, "w", encoding="utf-8") as handle:
            json.dump(mapper.names, handle, indent=2, sort_keys=True)
    print("masked: " + ", ".join(f"{k}={v}" for k, v in sorted(mapper.counts.items())), file=sys.stderr)


if __name__ == "__main__":
    main()

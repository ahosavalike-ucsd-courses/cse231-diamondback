import os
from collections import defaultdict
from sexpdata import loads

test_path = os.path.dirname(os.path.abspath(__file__))

def list_tests():
    for f in os.listdir(test_path):
        if f.endswith(".snek"):
            yield f

prop_key_lookup = {
    "dynamic": "expected",
    "static": "expected",
    "output": "expected",
    "input": "input",
}
prop_type_lookup = {
    "dynamic": "runtime_error_tests!",
    "output": "success_tests!",
    "static": "static_error_tests!",
}
def read_test(fn):
    print(f"reading {fn}")
    with open(os.path.join(test_path, fn)) as f:
        for (i,line) in enumerate(f.readlines()):
            if not line.startswith(";"):
                continue
            data = loads(line[1:])
            if type(data[0]) != list:
                data = [data]
            out = {
                "name": fn.removesuffix(".snek") + f"_{i}",
                "file": fn,
            }
            typ = None
            for entry in data:
                prop = entry[0]
                val = entry[1:]
                out[prop_key_lookup[str(prop)]] = "\\n".join(map(str, val))
                if typ is None:
                    typ = prop_type_lookup.get(str(prop))
            yield typ, out

def gentest(tests, dest):
    final_tests = defaultdict(list)
    for test in tests:
        for (typ, tst) in read_test(test):
            print(typ, tst)
            final_tests[typ].append(tst)
    with open(os.path.join(test_path, dest), "w") as f:
        f.truncate()
        f.write("mod infra;\n\n")
        for typ, tests in final_tests.items():
            f.write(f"{typ} {{\n")
            for test in tests:
                f.write("\t{\n")
                f.write(f'\t\tname: {test["name"]},\n\t\tfile: "{test["file"]}",\n')
                if test.get("input") is not None:
                    f.write(f'\t\tinput: "{test["input"]}",\n')
                f.write(f'\t\texpected: "{test["expected"]}",\n')
                f.write("\t},\n")
            f.write("}\n\n")

if __name__ == "__main__":
    sneks = list_tests()
    gentest(sneks, "all_tests.rs")

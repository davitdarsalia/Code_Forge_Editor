import re
import sys

RENAMES = {
    "variable.builtin": "variable.special",
    "variable.member": "property",
    "variable.parameter": "variable",
    "variable.parameter.builtin": "variable.special",
    "constant.builtin": "boolean",
    "constant.macro": "preproc",
    "function.macro": "preproc",
    "function.builtin": "function",
    "function.call": "function",
    "function.method": "function",
    "function.method.call": "function",
    "function.special": "function",
    "keyword.directive": "preproc",
    "keyword.directive.define": "preproc",
    "type.builtin": "enum",
    "type.definition": "type",
    "type.qualifier": "keyword",
    "module": "type",
    "module.builtin": "type",
    "namespace": "type",
    "comment.documentation": "comment.doc",
    "string.regexp": "string.regex",
    "string.special.url": "link_uri",
    "character": "number",
    "character.special": "variable.special",
    "tag.attribute": "attribute",
    "tag.delimiter": "punctuation",
    "tag.builtin": "tag",
    "annotation": "attribute",
    "decorator": "attribute",
    "conditional": "keyword",
    "repeat": "keyword",
    "include": "keyword",
    "exception": "keyword",
    "storageclass": "keyword",
    "define": "preproc",
    "macro": "preproc",
    "parameter": "variable",
    "field": "property",
    "method": "function",
    "float": "number",
    "symbol": "string.special.symbol",
    "markup.link": "link_text",
    "markup.link.label": "link_text",
    "markup.link.url": "link_uri",
    "markup.heading": "title",
    "markup.strong": "emphasis.strong",
    "markup.italic": "emphasis",
    "markup.underline": "emphasis",
    "markup.strikethrough": "emphasis",
    "markup.heading.1": "title",
    "markup.heading.2": "title",
    "markup.heading.3": "title",
    "markup.heading.4": "title",
    "markup.heading.5": "title",
    "markup.heading.6": "title",
    "markup.raw": "text.literal",
}
DROP = {"spell", "nospell", "none"}


def strip_comments(text):
    out = []
    for line in text.split("\n"):
        in_string = False
        escaped = False
        cut = len(line)
        for i, ch in enumerate(line):
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = not in_string
            elif ch == ";" and not in_string:
                cut = i
                break
        out.append(line[:cut])
    return "\n".join(out)


def forms(text):
    i, n = 0, len(text)
    result = []
    while i < n:
        while i < n and text[i].isspace():
            i += 1
        if i >= n:
            break
        start = i
        i = node_end(text, i)
        while True:
            j = i
            while j < n and text[j].isspace():
                j += 1
            if j < n and text[j] in "*+?" :
                i = j + 1
                continue
            if j < n and text[j] == "@":
                k = j + 1
                while k < n and (text[k].isalnum() or text[k] in "_.-"):
                    k += 1
                i = k
                continue
            break
        result.append(text[start:i].strip())
    return result


def node_end(text, i):
    ch = text[i]
    if ch == '"':
        i += 1
        while text[i] != '"':
            i += 2 if text[i] == "\\" else 1
        return i + 1
    if ch in "([":
        depth = 0
        while True:
            c = text[i]
            if c == '"':
                i = node_end(text, i)
                continue
            if c in "([":
                depth += 1
            elif c in ")]":
                depth -= 1
                if depth == 0:
                    return i + 1
            i += 1
    j = i
    while j < len(text) and not text[j].isspace() and text[j] not in "()[]":
        j += 1
    return j


def rename(form):
    def sub(match):
        name = match.group(1)
        if name in DROP:
            return ""
        return "@" + RENAMES.get(name, name)

    return re.sub(r'(?<![\w"\\])@([A-Za-z][\w.]*)', sub, form)


LUA_CLASSES = {"%u": "A-Z", "%l": "a-z", "%d": "\\\\d", "%a": "A-Za-z", "%w": "A-Za-z0-9"}


def lua_to_regex(form):
    def fix(match):
        pattern = match.group(2)
        for key, value in LUA_CLASSES.items():
            pattern = pattern.replace(key, value)
        return match.group(1) + '"' + pattern + '"'

    form = form.replace("#lua-match?", "#match?")
    return re.sub(r'(#match\?\s+@[\w.]+\s+)"((?:[^"\\]|\\.)*)"', fix, form)


def convert(sources):
    selected = []
    for source in sources:
        selected.extend(forms(strip_comments(source)))
    kept = []
    for form in selected:
        form = re.sub(r"\(#set![^()]*(\"[^\"]*\")?[^()]*\)", "", form)
        form = lua_to_regex(form)
        if re.search(r"has-ancestor\?|has-parent\?|#offset!|#vim-match", form):
            sys.stderr.write("unsupported: " + form[:80].replace("\n", " ") + "\n")
            continue
        names = re.findall(r'(?<![\w"\\])@([A-Za-z][\w.]*)', form)
        real = [n for n in names if n not in DROP and not n.startswith("_")]
        if names and not real and all(n in DROP for n in names):
            continue
        kept.append(rename(form))
    kept.reverse()
    return "\n\n".join(kept) + "\n"


if __name__ == "__main__":
    out = sys.argv[1]
    inputs = [open(p).read() for p in sys.argv[2:]]
    with open(out, "w") as handle:
        handle.write(convert(inputs))

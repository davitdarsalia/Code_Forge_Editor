import json
import plistlib
import sys

ROOT = "/Applications/Xcode.app/Contents/SharedFrameworks/DVTUserInterfaceKit.framework/Versions/A/Resources/FontAndColorThemes/"


def parse(value):
    parts = [float(x) for x in value.split()]
    return parts


def hexa(parts, background=None):
    red, green, blue, alpha = parts
    if alpha < 1 and background is not None:
        red, green, blue = (c * alpha + b * (1 - alpha) for c, b in zip((red, green, blue), background))
    return "#%02X%02X%02X" % tuple(round(c * 255) for c in (red, green, blue))


def build(name, appearance, source):
    with open(ROOT + source, "rb") as handle:
        theme = plistlib.load(handle)
    syntax = {k: parse(v) for k, v in theme["DVTSourceTextSyntaxColors"].items()}
    background = parse(theme["DVTSourceTextBackground"])[:3]

    def color(key):
        return {"color": hexa(syntax[key], background)}

    def bold(key):
        return {"color": hexa(syntax[key], background), "font_weight": 700}

    def italic(key):
        return {"color": hexa(syntax[key], background), "font_style": "italic"}

    plain = color("xcode.syntax.plain")
    keyword = bold("xcode.syntax.keyword")
    string = color("xcode.syntax.string")
    comment = color("xcode.syntax.comment")
    system_type = color("xcode.syntax.identifier.type.system")
    entries = {
        "attribute": color("xcode.syntax.attribute"),
        "boolean": keyword,
        "comment": comment,
        "comment.doc": color("xcode.syntax.comment.doc"),
        "constant": color("xcode.syntax.identifier.constant.system"),
        "constructor": color("xcode.syntax.identifier.type"),
        "embedded": plain,
        "emphasis": {"font_style": "italic"},
        "emphasis.strong": {"font_weight": 700},
        "enum": system_type,
        "function": color("xcode.syntax.identifier.function"),
        "hint": comment,
        "keyword": keyword,
        "label": plain,
        "link_text": color("xcode.syntax.url"),
        "link_uri": color("xcode.syntax.url"),
        "number": color("xcode.syntax.number"),
        "operator": plain,
        "predictive": comment,
        "preproc": color("xcode.syntax.preprocessor"),
        "primary": plain,
        "property": color("xcode.syntax.identifier.variable"),
        "punctuation": plain,
        "punctuation.bracket": plain,
        "punctuation.delimiter": plain,
        "punctuation.list_marker": keyword,
        "punctuation.special": plain,
        "string": string,
        "string.escape": string,
        "string.regex": string,
        "string.special": string,
        "string.special.symbol": string,
        "tag": keyword,
        "tag.doctype": color("xcode.syntax.preprocessor"),
        "text.code.span": string,
        "text.literal": string,
        "title": bold("xcode.syntax.declaration.type"),
        "type": color("xcode.syntax.identifier.type"),
        "variable": plain,
        "variable.special": keyword,
        "variant": color("xcode.syntax.identifier.constant"),
    }
    return {
        "name": name,
        "appearance": appearance,
        "style": {
            "editor.background": hexa(background + [1.0]),
            "editor.foreground": plain["color"],
            "editor.active_line.background": hexa(parse(theme["DVTSourceTextCurrentLineHighlightColor"])),
            "editor.invisible": hexa(parse(theme["DVTSourceTextInvisiblesColor"])),
            "syntax": entries,
        },
    }


for name, appearance, source, target in (
    ("Xcode Default (Dark)", "dark", "Default (Dark).xccolortheme", "xcode-dark.json"),
    ("Xcode Default (Light)", "light", "Default (Light).xccolortheme", "xcode-light.json"),
):
    with open(sys.argv[1] + "/" + target, "w") as out:
        json.dump(build(name, appearance, source), out, indent=2)
        out.write("\n")

# Builds the compact emoji table bundled with Swiftcord.ViewModels from emojibase-data 17.0.0 (MIT).
#
# Download into DIR from https://cdn.jsdelivr.net/npm/emojibase-data@17.0.0/en/:
#   compact.json, data.json, messages.json, shortcodes/iamcal.json, shortcodes/github.json
# then run:
#   python3 build/make_emoji.py DIR src/Swiftcord.ViewModels/Emojis/emoji.json
import json, sys
src, out = sys.argv[1], sys.argv[2]
compact = json.load(open(f"{src}/compact.json"))
iamcal = json.load(open(f"{src}/iamcal.json"))
github = json.load(open(f"{src}/github.json"))
messages = json.load(open(f"{src}/messages.json"))
# type 0 = text presentation by default, which needs U+FE0F to show as emoji (and is how Discord names it).
presentation = {e["hexcode"]: e.get("type", 1) for e in json.load(open(f"{src}/data.json"))}

def codes(hexcode):
    out = []
    for table in (iamcal, github):
        v = table.get(hexcode, [])
        for c in ([v] if isinstance(v, str) else v):
            if c not in out: out.append(c)
    return out

def text(hexcode):
    """The fully-qualified sequence, as Discord expects in reactions."""
    s = "".join(chr(int(h, 16)) for h in hexcode.split("-"))
    if "-" not in hexcode and presentation.get(hexcode) == 0:
        s += "\ufe0f"
    return s

groups = [g for g in sorted(messages["groups"], key=lambda g: g["order"]) if g["key"] != "component"]
index = {g["order"]: i for i, g in enumerate(groups)}
rows = []
for e in sorted(compact, key=lambda e: e.get("order", 1 << 30)):
    if e.get("group") not in index: continue
    sc = codes(e["hexcode"])
    name = sc[0] if sc else e["label"].replace(" ", "_").replace(":", "").lower()
    terms = " ".join(dict.fromkeys(sc[1:] + [e["label"]] + e.get("tags", [])))
    skins = [text(s["hexcode"]) for s in e.get("skins", []) if "-" in s["hexcode"] and len(s["hexcode"].split("-")) == 2]
    row = [text(e["hexcode"]), name, terms, index[e["group"]]]
    if len(skins) == 5: row.append(skins)
    rows.append(row)
json.dump({"source": "emojibase-data 17.0.0 (MIT, Miles Johnson)", "groups": [g["message"].title().replace("&", "&") for g in groups], "emoji": rows},
          open(out, "w"), ensure_ascii=False, separators=(",", ":"))
print(len(rows), "emoji in", len(groups), "groups")

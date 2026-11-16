"""List relics/cards/potions defined by decompiled StS2 mods, with the ModelId entry the game will use.

Rules (from decompiled game + BaseLib):
  entry = Slugify(ClassName)                       (game: ModelDb.GetEntry)
  if [CustomID("X")]  -> entry = X                 (BaseLib PrefixIdPatch)
  elif class implements BaseLib ICustomModel      -> entry = ROOTNAMESPACE + "-" + entry
Usage: python -I scan_content.py <decomp_root>
"""
import json, re, sys
from pathlib import Path

root = Path(sys.argv[1])
CATS = {"RelicModel": "RELIC", "CardModel": "CARD", "PotionModel": "POTION"}
# BaseLib bases that implement ICustomModel
CUSTOM_BASES = {"CustomRelicModel", "CustomCardModel", "CustomPotionModel", "ConstructedCardModel"}

cls_re = re.compile(r"^\s*(?:public|internal)?\s*(?:sealed\s+|abstract\s+|partial\s+)*class\s+(\w+)(?:<[^>]*>)?\s*:\s*([\w.<>]+)", re.M)
ns_re = re.compile(r"^namespace\s+([\w.]+)", re.M)
cid_re = re.compile(r'\[CustomID\("([^"]+)"\)\]\s*(?:\[[^\]]*\]\s*)*(?:public|internal)?\s*(?:sealed\s+|abstract\s+)*class\s+(\w+)')

def slugify(name):
    s = re.sub(r"(?<=[A-Za-z0-9])(?=[A-Z])", "_", name.strip()).upper()
    return re.sub(r"[^A-Z0-9_]", "", s)

result = {}
for mod in sorted(p for p in root.iterdir() if p.is_dir() and p.name != "BaseLib"):
    classes = {}  # name -> (base, namespace, abstract, customid)
    for f in mod.rglob("*.cs"):
        text = f.read_text(encoding="utf-8", errors="replace")
        nsm = ns_re.search(text)
        ns = nsm.group(1) if nsm else ""
        cids = {c: i for i, c in cid_re.findall(text)}
        for m in cls_re.finditer(text):
            name, base = m.group(1), m.group(2).split("<")[0].split(".")[-1]
            abstract = "abstract" in m.group(0).split("class")[0]
            classes[name] = (base, ns, abstract, cids.get(name))

    def resolve(name, seen=()):
        """-> (category, is_custom) or None"""
        base = classes.get(name, (name,))[0] if name in classes else name
        if base in CATS:
            return CATS[base], False
        if base in CUSTOM_BASES:
            return CATS[base.replace("Custom", "").replace("Constructed", "")], True
        if base in classes and base not in seen:
            r = resolve(base, seen + (name,))
            return r
        return None

    items = []
    for name, (base, ns, abstract, cid) in classes.items():
        r = resolve(name)
        if not r or abstract:
            continue
        cat, custom = r
        entry = slugify(name)
        if cid:
            entry = cid
        elif custom:
            entry = ns.split(".")[0].upper() + "-" + entry
        items.append({"category": cat, "class": name, "id": f"{cat}.{entry}"})
    if items:
        result[mod.name] = sorted(items, key=lambda i: (i["category"], i["id"]))

json.dump(result, sys.stdout, indent=1)

import json

JSON = "/Users/fanis/Library/Application Support/com.apple.wallpaper/aerials/manifest/entries.json"
with open(JSON) as f:
    d = json.load(f)
for a in d.get("assets", []):
    sid = a.get("shotID", "")
    if sid.startswith("PHONTO_"):
        a["includeInShuffle"] = True
        a["pointsOfInterest"] = {"0": f"{sid}_0"}
        print(
            f"patched {sid}: includeInShuffle=True, pointsOfInterest={a['pointsOfInterest']}"
        )
with open(JSON, "w") as f:
    json.dump(d, f, indent=2)

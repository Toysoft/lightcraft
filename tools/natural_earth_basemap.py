#!/usr/bin/env python3
"""Build LightCraft's offline basemap (assets/map/natural-earth-50m.bin) from Natural Earth.

Natural Earth (naturalearthdata.com) is in the public domain. This reads three of its 1:50m
GeoJSON files (land, lakes, land boundary lines) and writes their outlines in the compact format
`crates/map/src/basemap.rs` reads:

    8 bytes   magic "LCMAP1\\0\\0"
    3 layers  land rings, lake rings, boundary lines; each:
        u32   number of paths
        each  u32 number of points, then per point i16 longitude, i16 latitude
              (degrees * 32767 / 180 and degrees * 32767 / 90, rounded)

All integers are little-endian. Points that round to the same value as the one before are dropped.

    python3 tools/natural_earth_basemap.py <folder with the .geojson files> assets/map/natural-earth-50m.bin

The GeoJSON comes from https://github.com/nvkelso/natural-earth-vector (tag v5.1.2), folder
`geojson/`: ne_50m_land, ne_50m_lakes, ne_50m_admin_0_boundary_lines_land.
"""

import json
import struct
import sys
from pathlib import Path

LAYERS = ["ne_50m_land", "ne_50m_lakes", "ne_50m_admin_0_boundary_lines_land"]


def paths(geometry):
    kind, coords = geometry["type"], geometry["coordinates"]
    if kind == "LineString":
        return [coords]
    if kind in ("Polygon", "MultiLineString"):
        return coords
    if kind == "MultiPolygon":
        return [ring for polygon in coords for ring in polygon]
    raise SystemExit(f"unexpected geometry {kind}")


def quantise(path):
    out = []
    for lon, lat in (p[:2] for p in path):
        q = (round(max(-180.0, min(180.0, lon)) * 32767 / 180), round(max(-90.0, min(90.0, lat)) * 32767 / 90))
        if not out or out[-1] != q:
            out.append(q)
    return out


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    source, target = Path(sys.argv[1]), Path(sys.argv[2])
    blob = bytearray(b"LCMAP1\0\0")
    for layer in LAYERS:
        features = json.loads((source / f"{layer}.geojson").read_text())["features"]
        kept = [q for f in features for q in map(quantise, paths(f["geometry"])) if len(q) >= 2]
        blob += struct.pack("<I", len(kept))
        for q in kept:
            blob += struct.pack("<I", len(q))
            for lon, lat in q:
                blob += struct.pack("<hh", lon, lat)
        print(f"{layer}: {len(kept)} paths, {sum(map(len, kept))} points")
    target.write_bytes(blob)
    print(f"{target}: {len(blob)} bytes")


if __name__ == "__main__":
    main()

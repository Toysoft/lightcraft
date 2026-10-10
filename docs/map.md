# Map

LightCraft shows where photos were taken on a map drawn from raster map tiles. This page explains what the map
fetches, from where, how it follows the tile provider's rules, what that means for your privacy, and how to turn it
off.

Status: the map core is in place (`crates/map`: tile maths, tile sources, the tile cache and the fetch queue, plus the
`map.*` commands below). The map view, photo pins and the map in the Info panel come next; until then nothing in the
app asks for tiles, so nothing is fetched.

## What is fetched, and when

- **Only map tiles**: 256 × 256 px PNG images of the map, by zoom level and position
  (`https://tile.openstreetmap.org/{z}/{x}/{y}.png` by default). No photos, GPS positions, file names or account data
  are ever sent.
- **Only while a map is on screen**, and **only the tiles you are looking at**: the tiles the view draws plus at most
  one ring of tiles around them, at the zoom level shown. LightCraft never prefetches other zoom levels or areas, and
  has no "download this area for offline use". Nothing is fetched at startup.
- **Newest view first**: when you pan or zoom, tiles that left the view and haven't started are dropped; at most two
  requests run at once.
- **Gently**: after an error, a `429 Too Many Requests` or a server error, LightCraft waits (1 s, doubling up to
  5 minutes, and at least as long as the server's `Retry-After`) and then tries one request at a time until one
  succeeds. A tile the server refuses (e.g. 404) or that isn't a valid image isn't asked for again for 10 minutes.
- Tiles larger than 2 MB, images that aren't PNG / JPEG / WebP of at most 1024 px a side, and anything that fails to
  decode are errors (the map shows its placeholder), never a crash.

## OpenStreetMap's tile policy

The default tiles come from the OpenStreetMap Foundation's tile servers, which are run by volunteers and donations and
are free to use under the [OSMF tile usage policy](https://operations.osmfoundation.org/policies/tiles/). LightCraft
follows it:

- It uses the exact URL `https://tile.openstreetmap.org/{z}/{x}/{y}.png` (no subdomains, HTTPS only).
- It shows the attribution **"© OpenStreetMap contributors"** visibly on the map, never behind a toggle or off
  screen. (The map data is © OpenStreetMap contributors, available under the
  [Open Database License](https://www.openstreetmap.org/copyright).)
- Every request carries a distinct User-Agent naming the app, its version and where to find it:
  `LightCraft/<version> (+https://github.com/storytold/lightcraft)`.
- It honours the server's caching headers (`Cache-Control: max-age`, `Expires`, `ETag`, `Last-Modified`); when the
  server gives none, a tile is kept fresh for 7 days. Expired tiles are shown while LightCraft asks the server whether
  they changed (`If-None-Match` / `If-Modified-Since`; a `304 Not Modified` answer costs no download). It never sends
  `Cache-Control: no-cache` or `Pragma: no-cache`.
- It doesn't bulk-download, prefetch or offer offline areas: only the tiles in view (plus one tile around) are fetched.

Access is best effort: the OSMF can limit or block any app without notice. If that happens, use another tile server
or offline mode (below).

## Choosing the tile server

The tile source is a library preference (saved in the library's `prefs.json`), set with the `map.tileSource`
command (and in Settings once the map view lands):

| Params | Source |
|---|---|
| `{}` | Report the current source: `{id, urlTemplate, attribution, maxZoom, userAgent, offline}` |
| `{id: "osm"}` | OpenStreetMap's standard tiles (the default; max zoom 19) |
| `{offline: true}` | Offline: no network at all, no tiles; the map shows only what LightCraft draws on it |
| `{urlTemplate, attribution, maxZoom?, id?}` | Another raster tile server, e.g. one you host. The template must start with `https://` and contain `{z}`, `{x}` and `{y}` exactly once (no other placeholders); the attribution is required (it is shown on the map); `maxZoom` is 0–24 (default 18); `id` names its cache folder (`[a-z0-9._-]`, derived from the template when absent) |

Fields given alone change the current source (e.g. `{maxZoom: 16}`). Anything that doesn't pass these checks is
refused with an error and changes nothing; a hand-edited `prefs.json` with an unusable source falls back to
OpenStreetMap. Respect the terms of any server you configure: many commercial providers need an API key and forbid
some uses, and Google, Apple, Bing and Esri imagery may not be used this way.

## Privacy

Looking at the map tells the tile server (the OpenStreetMap Foundation, by default) which areas you look at, at
which zoom, from your IP address, like any web map does; see the OSMF's
[privacy policy](https://osmfoundation.org/wiki/Privacy_Policy). Your photos and their GPS positions stay on your
computer: pins are drawn locally. If you would rather nothing about your map views left your computer, choose
offline mode (`map.tileSource {offline: true}`) or a tile server you run yourself.

## The tile cache

Downloaded tiles are kept on disk so the same area isn't downloaded again:

| System | Folder |
|---|---|
| macOS | `~/Library/Caches/LightCraft/map-tiles` |
| Windows | `%LOCALAPPDATA%\LightCraft\Cache\map-tiles` |
| Linux and others | `$XDG_CACHE_HOME/lightcraft/map-tiles` (else `~/.cache/lightcraft/map-tiles`) |

`LIGHTCRAFT_MAP_CACHE` overrides the folder. Inside it every tile source has its own folder (`osm`, `custom-…`, the
id you gave), holding one `<z>/<x>/<y>.tile` file per tile (the image as the server sent it, with its caching
metadata). The cache is capped at 500 MB for all sources together: past that, the tiles used least recently are
deleted. Tiles are written atomically (a `.part` file renamed into place), and a damaged cache file is deleted and
fetched again. The app, `lightcraft-cli` and the MCP server share the folder.

- `map.cacheInfo` → `{bytes, tiles, capBytes, dir}`
- `map.clearCache` deletes every cached tile (all sources) → what was deleted. Only `.tile` / `.part` files in the
  cache's own layout are deleted, so pointing `LIGHTCRAFT_MAP_CACHE` somewhere odd can't delete anything else.

The web build has no tile cache and fetches no tiles yet.

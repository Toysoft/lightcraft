# Map view

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** minor (deliberate differences from Lightroom Classic written down; the photo limit is a setting; the map asks once about OpenStreetMap; the filmstrip's photos, not the selection; a click on a marker selects its photos) · **Target:** Adobe Lightroom Classic 15.5 (Map module)

**View ▸ Map** (`view.map`, the pin in the bottom bar) shows where the photos of the current view were taken.

## What it shows

- **The filmstrip decides.** The map shows the photos the grid and the filmstrip list (the folder, album or filter
  result in view) that have a GPS position, whatever is selected. Another folder or filter, and the map follows and
  fits itself to the new photos.
- **At most 100 photos** by default: the first 100 of them, in filmstrip order, that have a position. **Settings ▸
  Interface ▸ Map** offers 50, 100, 250, 500 or 1000 (`lightcraft_map::PHOTO_LIMITS`; more markers take longer to
  draw, and 1000 is the most, `MOST_PHOTOS`). The line above the map says how many are shown, how many have no position and when the limit left some out.
- **Markers.** A marker is the photo's picture in a frame with a tip at its lower edge; the tip's point is the
  position.
- **Stacks.** Photos at the same place are one marker: the first photo's picture, with the number of photos white on
  a black disc in its lower right corner. Photos closer than 44 points on screen are stacked too, so zooming out
  merges markers and zooming in separates them; photos at exactly the same position stay one stack at every zoom.
- **Clicks.** A click on a marker selects its photo, or all photos of its stack: they are marked in the filmstrip
  below the map, the first one is the active photo (Info, Edit and the rating bar then act on it), and the marker's
  frame takes the accent colour. The map does not move. Hovering lists the file names and the position.
- **Moving.** Drag to pan, wheel or pinch to zoom about the pointer, double click to zoom in, `+` / `−` / **Fit**
  above the map.

## The map itself

| | Offline (default) | OpenStreetMap (opt-in) |
|---|---|---|
| Shows | land, lakes, country borders | streets, place names, terrain detail |
| Data | Natural Earth 1:50m, public domain, built in (`assets/map/natural-earth-50m.bin`, 405 kB) | tiles from `tile.openstreetmap.org`, © OpenStreetMap contributors (ODbL) |
| Network | none | one request per tile not yet on disk |
| Web build | yes | no |

The offline map has no detail below country level: zoomed into a city it is plain land with the markers on it.

**Online tiles are off until switched on** (the **OpenStreetMap** button above the map, View ▸ Online Map, or
`map.onlineTiles`); the choice is saved with the interface settings. The first time the map is open it asks once
(**Use OpenStreetMap** / **Keep the Offline Map**, a card over the map's lower edge); either answer, or the switch,
ends the question. With them on:

- The tile server learns which part of the map is viewed (the tile addresses) and the computer's IP address. The
  photos, their file names and their exact positions are not sent.
- Tiles are kept in `<settings folder>/map-tiles/z/x/y.png` and reused for 30 days, so a place looked at before costs
  no request. A tile older than that is fetched again, and still used if the server cannot be reached.
- At most two requests run at once and only tiles in view are fetched, following the
  [tile usage policy](https://operations.osmfoundation.org/policies/tiles/). Requests identify LightCraft in the
  `User-Agent`.
- While a tile loads, its place shows what other zoom levels already have: the covering part of a coarser tile
  (up to six levels up), or the finer tiles inside it after zooming out. The map sharpens tile by tile; where nothing
  is there yet, the light offline map shows, in the dark theme too.
- Loading shows as "Loading map tiles" in the activity stack and can be cancelled there. Tiles that have not arrived
  show the offline map; if none can be loaded the map says so and stays on the offline map.

## Commands

| Command | Does |
|---|---|
| `view.map` | switch to the Map view |
| `map.photos {ids?, limit?, lat?, lon?, zoom?, width?, height?, radius?}` | engine query (also CLI and MCP): the photos on the map and their stacks for a view; without a centre the view is fitted to the photos; `limit` defaults to 100 (the Map view passes its setting) |
| `map.view {lat?, lon?, zoom?, fit?}` | move the map of the running interface; returns the view and its stacks |
| `map.onlineTiles {enabled?}` | switch OpenStreetMap tiles on or off (no argument: toggle) |

Control ids: `map:canvas`, `map:stack:<n>` (markers, in filmstrip order; the lower edge's middle is the position), `button:map-fit`, `button:map-zoom-in`,
`button:map-zoom-out`, `button:map-online`; the question: `map:ask-online`, `button:map-ask-online-yes`,
`button:map-ask-online-no`.

## Code

- `crates/map` (L1): Web Mercator viewport, fit, tile addresses, stacking, and the basemap renderer. No UI, no network.
- `crates/engine/src/cmd/map.rs`: which photos are shown; `map.photos`.
- `crates/engine/src/map_tiles.rs`: the online tile store (disk cache, two workers, activity row).
- `crates/ui-egui/src/panels/map.rs`: the view.
- `tools/natural_earth_basemap.py`: rebuilds the basemap file from Natural Earth's GeoJSON.

## Differences from Lightroom Classic

The target is Lightroom Classic's Map module. These differences are deliberate. The comparison is from knowledge of
how Lightroom Classic behaves, not from a side-by-side test with a running Lightroom; whoever compares the two
should correct this list.

| | Lightroom Classic | LightCraft | Why |
|---|---|---|---|
| Place in the app | a module of its own (Library, Develop, **Map**, …) | a view in the single window (View ▸ Map, the pin in the bottom bar) | LightCraft follows Lightroom's single-window layout and has no Classic module switcher, by design ([ui-parity](ui-parity.md#assessment-by-area)) |
| Markers | balloons, a group shows its number | the photo's picture with a tip at its position; a stack shows its number on a black disc | the picture says which photo it is without hovering; the markers are our own drawing, not Adobe's |
| How many photos | no limit | 100 by default, 50 to 1000 in Settings ▸ Interface ▸ Map | every marker is a thumbnail, and stacking compares each photo with each stack: a limit keeps the map quick on large folders; the line above the map says when it leaves photos out |
| Map source | an online map service: no map without a connection | a built-in offline world map (Natural Earth), OpenStreetMap tiles when switched on | the map works offline and fetches nothing unasked; the tile server learns which area is viewed, so it is the user's choice (asked once) |
| Map styles | several (road, satellite, terrain, light, dark …) | the offline map (following the light or dark theme) or OpenStreetMap's standard style | OpenStreetMap's standard tiles are the freely usable source; satellite imagery needs a commercial provider |
| Attribution | the map provider's | "Made with Natural Earth" or "© OpenStreetMap contributors" | required by the OpenStreetMap licence, asked for by Natural Earth |

## Not there yet

Dragging photos onto the map to geotag them, saved locations, drawing a GPX track, a location filter, searching for a
place, and a finer offline map (the 1:10m Natural Earth data, or a downloadable region extract).

## Revision history

| Date | Change | What |
|---|---|---|
| 2026-10-13 | major | Created with the Map view |
| 2026-10-14 | minor | The map shows the filmstrip's photos and a click selects a marker's photos; markers have a tip and a black count disc; asks once about OpenStreetMap; tiles of other zoom levels stand in while tiles load; photo limit in Settings; deliberate differences from Lightroom Classic |

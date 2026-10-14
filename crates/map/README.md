# lightcraft-map

The map view's model, without any UI or network code (L1):

- `View`: a Web Mercator viewport (centre, zoom, size in points) with pan, zoom about a point and
  fit-to-photos. Every input is sanitised: NaN, infinities and absurd sizes give a usable view.
- `stacks`: photos that would overlap on screen at the current zoom are shown as one stack. Photos
  at the same position are always one stack, at any zoom.
- `View::tiles`: the slippy-map tiles (`z/x/y`) covering the view and where each one is drawn.
- `basemap`: the offline world map (land, lakes, country borders) rendered to RGBA for a view.

The basemap data is `assets/map/natural-earth-50m.bin`, built from Natural Earth 1:50m (public
domain) by `tools/natural_earth_basemap.py`; see `assets/ATTRIBUTION.md`.

Sources: Web Mercator and slippy-map tile numbering (OpenStreetMap wiki, prose); scanline polygon
fill (textbook); the stacking rule is our own design.

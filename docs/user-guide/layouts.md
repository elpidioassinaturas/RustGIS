# Layouts and Reports

## Layouts

<figure markdown="span">
  ![A landscape A4 page layout with a map frame, title, legend, north arrow and scale bar, and the Publish tab open](https://assets.rustgis.app/images/layout.webp)
  <figcaption>A page layout with a map frame, title, legend, north arrow and scale bar.</figcaption>
</figure>

Home › New Layout adds a page layout (Letter to A0, landscape or portrait) with a map frame,
title, legend, north arrow, scale bar and a dynamic credits line. Layouts open as tabs next to
the maps; while one is open, its groups lead the Publish tab:

- **Map frames**: rectangle, circle and ellipse frames, or polygon and lasso frames drawn on the
  page. Each frame shows a map of the project at its own centre and scale. Turn on **Pan/Zoom
  Map** (Map Frames group, or a frame's properties) to drag inside frames to pan their maps and
  turn the wheel to zoom them about the pointer; Esc turns it off. Without it, Shift-drag a frame
  to pan its map. Double-click a frame to match the map view, or set the scale in its properties.
  A frame can be turned and tilted like the map view: a new frame, or one matched to the view,
  takes the view's bearing and pitch, and **Bearing** and **Pitch** in its properties (or
  `layout.update` with `bearing` and `pitch`) set them. Its north arrows turn with it. **Grid** draws a labelled graticule over a frame; **Extent Indicator** outlines another frame's
  extent (an overview map).
- **Extent indicators**: the **Extent Indicator** section of an overview frame's properties sets
  the outline's **Color** and **Line width** (red and 2 pt to begin with), so it can sit quietly on a
  muted page, and **Remove Extent Indicator** takes it away. Scripts set the same with `color`
  and `width` on `layout.addElement` (`kind` `extentIndicator`), or `layout.update` with
  `"extentIndicator": {"color": "#555555", "width": 1}` (`false` removes it).
- **Grids**: the **Grid** section of a frame's properties turns the grid on and sets its
  **Interval** in degrees (**Auto** picks one from the frame's extent), its **Style** (full lines
  across the frame, ticks along its neatline only, or both, with the **Tick length**), its colour
  and **Line width**, and its labels: decimal degrees (`112.5°W`, as many decimals as the
  interval needs) or degrees, minutes and seconds (`112°30′W`), with a hemisphere letter or a
  sign (`-112.5°`), on any of the four edges (longitudes along the top and bottom, latitudes along
  the left and right; the bottom and left to start with), and the side labels horizontal or
  running along the edge. From a command:
  `layout.addElement={"kind":"grid","step":0.5,"style":"ticks","format":"dms","edges":["top","bottom","left","right"],"labelOrientation":"vertical"}`
  (given again, it changes only the options it names), or `layout.update` with
  `"grid": {…}` (`false` removes the grid).
- **Map surrounds**: north arrow, scale bar, legend, colorbar, table frame (a layer's attribute table) and
  chart frame (a bar chart of a numeric field), plus a neatline or backdrop. A north arrow's
  properties pick its **Style** (classic arrow, compass rose, triangle or star) and its **Fill**
  and **Outline** colours (`"style":"compass","fill":"#B91C1C"`). A scale bar's pick its
  **Style** (alternating bar, stepped line or single line), its **Units** (metres or kilometres
  to suit the scale, or kilometres, metres, miles or feet) and its **Divisions** (1 to 10), each
  labelled where there is room (`"style":"line","units":"miles","divisions":2`). A legend shows what
  its frame draws: only layers with a feature at the frame's scale (through their definition
  query, display filters and scale range) and, for Unique Values, only the classes those features
  have, with `<all other values>` only when some feature falls in it and its symbol is not fully
  transparent. Graduated Colors and
  Proportional Symbols list their classes as they are (they are taken from the kept features when
  the symbology is applied). A layer that draws
  nothing there, such as an empty one, is left out, in layouts saved before this too. Tick **All
  classes** in its properties (`"allClasses": true`) to list every class of every visible layer
  instead, and **Hide other values** (`"hideOther": true`) to leave `<all other values>` out of
  the legend in either mode (the features it stands for are still drawn). Its **Layers** section
  has a checkbox for each layer of the frame's map: untick one to leave it out of the legend
  while the map still draws it (a mask, a highlight, a basemap-like layer). Scripts give the same
  with `"exclude": ["Graticule"]` on `layout.addElement` or `layout.update` (`[]` lists every
  layer again); a renamed layer stays left out.
- **Colorbar**: the colour scale of a raster layer in the frame's map, as the map draws it (its
  stretch, colour ramp and enhancement): a gradient bar for a stretch or a tinted hillshade,
  steps labelled at their breaks for Classify, and a block per value for Unique Values and colour
  tables. Its properties pick the **Layer** (the top visible raster unless one is picked), the
  **Title** (the units, such as `Elevation (m)`; the layer's name to start with), the
  **Orientation**, which side the **Labels** go on, how many **Ticks** a gradient labels (2 to
  21, the ends included) and **Reverse** (high values first). An RGB raster or a hillshade
  without a tint has no colour scale and shows a placeholder, as does a picked layer that was
  renamed or removed (it is kept by name, like a table or chart frame's layer) until another is
  picked. From a command:
  `layout.addElement={"kind":"colorbar","layer":"DEM","title":"Elevation (m)","orientation":"vertical","ticks":3}`.
- **Shapes**: **Rectangle**, **Circle**, **Ellipse** and **Line** (horizontal, vertical or
  either diagonal) add a shape in the middle of the page, to frame, highlight or divide parts of
  it. A circle is an ellipse in a square box. The properties set a shape's fill, outline and
  outline width (0 for none; a fill with no opacity leaves only the outline), and a line's colour,
  width, arrowheads and angle. From a command:
  `layout.addElement={"kind":"line","dir":"falling","color":"#C0392B","width":2}`.
- **Arrows**: **Arrow** adds an arrow pointing right. Drag the round handle above a selected arrow
  or line to turn it about its centre: it snaps to level, upright and diagonal within 3°, and
  **Shift** turns it in 15° steps. **Angle** in its properties sets it exactly (0° points right,
  90° up), and **Arrowheads** puts heads at its end, its start, both or neither, so any line can
  be an arrow. From a command:
  `layout.addElement={"kind":"arrow","angle":30,"length":60}`, or
  `layout.update={"element":7,"angle":-90}` to turn one.
- **Text** and **Dynamic Text**: `{date}`, `{time}`, `{scale}`, `{map}`, `{crs}`, `{page}`,
  `{pages}`, `{pageName}` and `{layout}` fill in when the page is drawn; the Format pane's
  **Insert Dynamic Text** menu, under the text, lists each with what it shows. The text, its
  size (2 to 400 points, as text graphics on the map), colour, bold, **Italic** (slanted, as water
  and ocean names are; the built-in fonts have no italic face, so the letters are slanted 12°),
  **Alignment** (left, center or right within the element) and **Letter spacing** in points
  (spaced-out names such as O C E A N O, or a negative value to tighten) are the same form a text
  on the map has in the pane; Enter ends the text and Shift+Enter starts a new line. Scripts set them with `italic`, `align` and
  `letterSpacing` on `layout.addElement` and `layout.update`. **Picture** places a PNG or JPEG.
- **Map Series**: one page per feature of an index layer, with the frame zoomed to each feature;
  step through pages, refresh, and export every page into one PDF.
- Click an element to select it, drag to move, drag its handles to resize, and edit it in the
  **Format** pane, which opens as a tab beside the other right-hand panes. Closing the pane keeps
  the element selected; selecting an element, or clicking the selected one, opens it again. The
  wheel zooms the page. **Delete** (or **Backspace**, the delete key on a
  Mac) deletes the selected element, and **Ctrl+D** (**⌘D** on a Mac) duplicates it: the copy is
  placed 5 mm down and to the right, on top, and selected. The **arrow keys** nudge it 1 mm, or
  10 mm with **Shift**, and keep it moving while held; a run of nudges is one step to undo.
- **Locked**, at the top of the Format pane, keeps a finished element from being changed by
  mistake: it can still be selected (a lock marks its corner, and it has no handles), but not
  moved, resized, deleted or edited, and a locked map frame's map isn't panned or zoomed. A map
  series still turns its pages. **Duplicate** makes an unlocked copy, and a map frame isn't
  deleted while one of its surrounds is locked. Scripts lock and unlock with `layout.update`
  `{"locked": true}` or `{"locked": false}`.
- **Rulers and guides**: rulers along the top and left show millimetres from the page's corner.
  Drag out of a ruler to add a guide, drag a guide to move it, and drag it back onto a ruler to
  remove it. Guides are saved with the layout but never printed or exported. **Clear Guides**
  removes them all, and **Rulers** in the Page group hides the rulers.
- **Snap**: a dragged or resized element snaps to the guides, the page's edges and centre, and
  the other elements' edges and centres, and the line it snapped to is highlighted. Hold Alt to
  place an element freely, or turn **Snap** off in the Page group.
- **Export** writes PDF (every map series page) or PNG; **Print** opens the PDF in the system
  viewer; **Save Layout File** and **Import Layout** move layouts between projects
  (`.rgislayout`).

## Reports

**Publish › New Report** lists the selected layer's records page after page; the **Report** pane
previews it and sets it up.

- **Data**: pick the detail **Fields** (or **Autofill Fields**), **Group** records by up to four
  fields (each group gets a heading and its own statistics), add **Statistics** (count, sum,
  mean, minimum, maximum), a **Dynamic Picture** per record (a field holding a PNG/JPEG path relative to the saved
  project's folder, or base64 image data), **Attachments** (a field listing each record's files,
  separated by `;`; image files show when they're inside the project's folder), and a **Chart Frame**
  (records, or a field's sum, per category).
- **Report Section**: **Supplemental Pages** of text before or after the records, **New
  Template** and **Existing Template** (a page layout drawn as a report page), a **Related
  Report** appended after this one, and **Subreports** (each record's related records through a
  relationship, listed beneath it).
- A where clause or the layer's selection limits the records; a sort field orders them within
  their groups.
- **Publish › Report**: **Export Report** writes a PDF (or the first page as PNG); **Save As Report
  File** keeps the report, and **Report Template** its design without the data source, to start
  reports on other layers (**Publish › Import Report**).

## Presentations and notebooks

**Publish › New Presentation** starts a presentation with a title page; the **Presentation** pane
previews, edits and plays it.

- **Page**: a **Map** page shows the map view as it is now, a **Blank** page shows text on a
  coloured page, an **Image** page a picture with a caption, and a **Video** page a video file or
  URL with an optional poster picture.
- Each page has a title, speaker notes and a duration (for **Play**), and can move up or down.
- **Publish › Presentation › Export Presentation** writes a self-contained HTML slideshow (arrow
  keys, `N` for notes, `P` to play; videos play in place), a PDF, or the first page as PNG.
  **Save As Presentation File** keeps it to import into other projects.

**Analyze › New Notebook** starts a notebook of text and command cells; **Add and Open** opens a
RustGIS (`.rgisnb`) or Jupyter (`.ipynb`) notebook. A command cell runs one command per line
(`id {json}`, as in scripts) and shows each command's result beneath it. Notebooks save in
either format.

# Workspace

Tools for working across maps and views, and for setting up the app the way you like it.

- **Side by Side** shows a second map beside the active one; **Link Views** keeps maps at the same
  place and scale, and **Link Cursors** marks the pointer's place on the other map.
- **Browser View** shows the project's maps, layouts, tables, styles, tasks, models, folders and
  offline areas as cards (double-click to open); **Item Pop-ups** add hover details.
- **Pane Sets** save the panes you have open and bring them back. The **Workflow Manager** tracks
  jobs (new, in progress, on hold, done) with the area each covers.
- **Tasks** (Analyze › Tasks, or View › Tasks) steps through a task's instructions: read the current step and do it
  yourself, with **Show Me** to find where, or click **Run** to have it done.
    - **Show Me** points at the step's command:
        - a ribbon button gets its tab opened and is outlined for a few seconds;
        - a tool is outlined in the Analyze gallery, or opened, empty, in the Tools pane;
        - a symbology step opens the Symbology pane on its layer;
        - a command no button runs opens Command Search on its name.
    - **Record Task** (in the pane, or Analyze › Task) turns an exercise into a task: do it once, and
      each command that changes the project becomes a step. Panes, dialogs and moving the view
      aren't recorded, and a colour adjusted several times stays one step. **Stop Recording** makes
      the task, named as typed beside it, and opens it for editing. Opening or creating another
      project ends a recording without making a task.
        - Undo doesn't remove a recorded step; delete it with Edit Steps.
        - A task holds at most 200 steps, and a step's parameters at most 4 MB: add large data to
          the project before recording.
        - Steps keep their parameters as they ran, file paths included (a file you added, say), so
          check them before sharing a task. A command run on the selected layer names it, so the
          step replays on that layer whichever is selected then; where two layers share a name it
          replays on the first, so rename one first (the recording says so). A command holding a password, key or token (a database
          connection, a signed address) isn't recorded, nor is data added from a download, which is
          kept for the session only. A plain `key` parameter that doesn't read as an access key (a
          field or a lookup key, say) is recorded, and the recording warns that it might be one:
          delete that step before sharing the task if it is.
    - **Edit Steps** gives each step ▲ ▼ to reorder it, **Edit** for its title and instructions, and
      **Delete**. A recorded tool's step is named after the tool.
    - In the Browser pane, a click on a task opens its steps; **Run All Steps** is in its context
      menu.
    - A task from a file (`.rgistask`, or inside a project or a package) can't read or write files
      or run scripts. Its built-in analysis tools may still name their output layers with a plain
      name: one with `/`, `\`, `.` or `:` (a folder, an extension or a drive) makes the task count
      as writing files, so write `Buffer 1500 m` rather than `Buffer 1.5 km`. GeoLibre and plugin
      tools' outputs are refused.
- **Thumbnails** for the project and for each map, from the view or an image.
- **Animation**: add keyframes (views), play them, and export the moves as PNG frames.
- **Time Slider** (View › Time) shows data through time, in a timeline docked under the map.

    <figure markdown="span">
      ![The Time Slider under the map at 2018, showing a layer of dated events cumulatively, with a bar for each year](https://assets.rustgis.app/images/time-slider.webp)
      <figcaption>The Time Slider in cumulative mode: every event up to 2018 is drawn.</figcaption>
    </figure>

    - **Bind To Time Slider** (a layer's context menu, or the bar's **Layers** menu) picks the
      layer's date field: the one most of whose values read as dates. Text dates (`2015-03-01`,
      `2015-03-01T12:00:00Z`, `2015/3/1`), epoch milliseconds or seconds, and years (`1958`) all
      read; times are UTC. Only the features whose date falls in the slider's window draw (and
      show in the legend). The bar's bars count each step's features.
    - **Step** is an hour, a day, a month or a year, times a number; the slider starts in steps
      that suit the data's span. **Show** sets the window: **This step**, **Wide** (a step either
      side), **Wider** (three either side), or **Cumulative** (everything up to the step).
    - Play, pause, step, or click and drag the timeline; **Loop** starts over at the end, and the
      speed runs from 0.5× to 4× (0.8 s a step at 1×).
    - Imagery by date: a tile address with date placeholders (`{date:YYYY-MM-DD}`, `{YYYY}`,
      `{MM}`, `{DD}`, `{YYYYMMDD}`) follows the slider. Add it with `map.addTiles`, or a
      cloud-optimised GeoTIFF a file per date with `layer.addUrl`, giving its `start` and `end`.
      For example NASA GIBS daily MODIS imagery:
      `https://gibs.earthdata.nasa.gov/wmts/epsg3857/best/MODIS_Terra_CorrectedReflectance_TrueColor/default/{date:YYYY-MM-DD}/GoogleMapsCompatible_Level9/{z}/{y}/{x}.jpg`.
    - A Zarr variable added with **Follow the time slider** (Add Data › Zarr…) shows the step the
      slider is at; see [Zarr stores](data.md#zarr-stores).
    - From scripts: `time.bind` (`"layer"`, `"field"`, `"start"`, `"end"`, `"unit"`, `"interval"`, `"show"`), `time.set` (the
      window: `"start"`, `"end"`, `"unit"`, `"interval"`, `"show"`, `"speed"`, `"loop"`,
      `"format"`…), `time.goTo` (`"date"`, or `"step"` from 0), `time.step` (`"by"`, negative goes
      back), `time.fields` (a layer's date fields) and `time.unbind`:
      `rustgis-cli run --cmd 'layer.addData={"path":"quakes.csv"}' --cmd 'time.bind={"layer":"quakes","unit":"year","show":"cumulative"}' --cmd 'time.goTo={"date":"2018"}'`.
    - **Export** writes the steps as an animated GIF (`time.export` also writes a zip of PNG
      frames). The slider, its window and the bindings are saved with the project; the off button
      turns it off and every feature shows again.
- **Time Series**: a Zarr variable's values through time at points clicked on the map, as lines on
  one chart; see [Zarr stores](data.md#zarr-stores).
- **Profile**: elevation profiles along clicked points or selected lines, auto-completed to the
  raster's edge, or across a line (a cross section), with gain, loss and a chart.
- **Infographics** (Home tab) summarise what's in view: counts, totals and top categories.
- **Offline**: Download Map takes the data in view into an offline area; open it to edit, then
  Sync sends changed, new and deleted features back and refreshes it. **Feature Cache** prepares
  every layer's geometry for the current projection (Fill, Empty, Cancel, Auto Cache).
- **Help**: choose the ribbon's tabs and the Quick Access Toolbar's buttons, open the user guide,
  and watch the **Diagnostic Monitor** (frame rate, draw time, data size).
- **Plugins**: **Plugins › Plugin Manager** installs, updates and removes [plugins](../plugins.md)
  from the registry at [plugins.rustgis.app](https://plugins.rustgis.app) or from a file.
- **Language**: **Settings › Language** switches the interface among
  [16 languages](../i18n.md) and chooses the [worldview](maps.md#worldview) of the built-in
  borders.
- **Start-up**: **Settings › Start-up** sets what RustGIS opens with: **Start With** the World Atlas
  sample or a blank map, or, in the desktop app, **Last Project** (the `.rgis` project last opened
  or saved) or **Choose Project…** (one you pick once), the **Projection** the map opens in (a
  preset or a supported projected EPSG or ESRI code, such as `3010` or `ESRI:102033`), and the **Extent** it opens on (the
  full extent, or **Use Current View** to keep the view you are looking at). A project opened at
  start keeps its own projection and view; the Projection and Extent settings apply to the sample
  and a blank map. If the project no longer opens (moved or deleted), RustGIS starts with the sample
  and says why. After a crash it starts with the sample too, in case the project is what crashed;
  starting `rustgis --sample` also skips the start-up project. The choices are remembered on this
  computer, or in this browser for the web app. Opening files from the command line starts a blank
  map.

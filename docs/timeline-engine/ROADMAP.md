# Timeline Engine roadmap

## Done

* Exact time (ticks), frame rates, drop-frame timecode
* Projects, sequences, video and audio tracks, clips, markers, link groups
* Overwrite and insert placement, split, delete, ripple delete (off / track / sync), move, trim, ripple trim, roll, slip, slide
* Undo, redo, grouped edits, dirty tracking
* Versioned, validated, atomically saved project files
* Queries a player needs: layers and sounds at a time
* Command line editing

## Not built yet (deliberately left for the engines that own them)

| Item | Belongs to |
|---|---|
| Keyframes (volume, position, opacity over time) | Keyframe |
| Effects and colour on clips | Effects |
| Opacity, blend modes, transforms | Compositing |
| Audio levels, pan, fades, mixing | Audio |
| Nested sequences (a sequence used as a clip) | Next step for this engine once Render exists |
| Copy / paste of clips between sequences | This engine, with the editor UI |
| Multicam, subclips, proxies per clip | Later |
| Import / export of other timeline formats (EDL, FCPXML, AAF) | Plugin |

## Next for this engine

* Nested sequences and compound clips
* Clip copy and paste (needs a clip clipboard type)
* Gap clips and "close all gaps"
* Snapping helpers (nearest edit point or marker to a time)
* Wider benchmarks on very large timelines (10,000+ clips)

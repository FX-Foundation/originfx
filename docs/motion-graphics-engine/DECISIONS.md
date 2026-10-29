# Decisions made while building the Motion Graphics Engine

Defaults chosen while building this engine, each with the way to reverse it. Earlier engines:
[Timeline T](../timeline-engine/DECISIONS.md), [Render R](../render-engine/DECISIONS.md),
[Video Editing V](../video-editing/DECISIONS.md), [Audio A](../audio-engine/DECISIONS.md),
[Keyframe K](../keyframe-engine/DECISIONS.md), [Effects E](../effects-engine/DECISIONS.md),
[Compositing C](../compositing-engine/DECISIONS.md).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| G1 | The drawing is a separate crate; the timeline depends on it for the graphic types (with serde switched on) | Testable without a timeline; the project needs to know what a graphic is to save and check it | Move the types into the timeline crate |
| G2 | A graphic is an *asset* (a still with no file), not a new kind of clip | Every existing edit, transition, effect, look and keyframe works on it with no new code, and one definition can be reused by many clips | `Asset::generator`, `graphics.rs` |
| G3 | Changing a graphic changes every clip that uses it | Same as editing a master clip; to get two looks, add two graphics | `set_generated` in `ops.rs` |
| G4 | Graphics are drawn at the sequence's size and always centred; position, scale, rotation and opacity come from the clip's look | One way to move things (and animate them); no second set of position numbers | Add offsets to `Shape` and `Text` |
| G5 | Shapes come from signed distance functions, not polygon fills | Smooth anti-aliasing for free, exact area, simple code; round-trips through the same maths for stroke | `shape.rs` |
| G6 | Rect, ellipse, regular polygon and star only; no free paths yet | The shapes titles and lower thirds actually use; paths are a larger feature (see roadmap) | `ShapeKind` |
| G7 | Text uses `fontdue` and a font *file*; the project stores the path | A pure Rust rasteriser that behaves the same everywhere; same path rule as media files | `text.rs` |
| G8 | If no font is named, the first usual system font is used; if there is none, the render stops with a clear error | Titles work out of the box on most machines, never with a silently wrong font. **Bundling a default font is left to the project owner** (a visual choice, and a licence one: SIL OFL fonts such as Inter are the usual pick) | `Fonts::default_path` |
| G9 | Outline is the text grown by a round brush; the shadow is a hard offset copy | Legible titles on any picture with simple, predictable maths | `text.rs` |
| G10 | Text is laid out with the font's own metrics and kerning, lines at newlines, no wrapping | Predictable; wrapping to a box needs a text box concept (roadmap) | `layout` in `text.rs` |
| G11 | Colours are straight RGBA8, saved as hex text | Readable, hand-editable project files | `Colour` |
| G12 | A drawn graphic is rendered once per sequence size and reused for every frame | Text and gradients are slow to draw, and they do not change between frames | `Renderer::graphics` |
| G13 | Numbers are checked (sizes up to 1000 %, 3 to 32 sides, text up to 2000 characters, and so on) when a graphic is added, changed or loaded | Same rule everywhere; a bad file is refused, not drawn | `graphics::validate` |
| G14 | Project file format version 7; `generator` only written for graphics | Old projects keep working | `FORMAT_VERSION` |
| G15 | CPU, one thread | Correct and identical everywhere first | See [ROADMAP.md](ROADMAP.md) |

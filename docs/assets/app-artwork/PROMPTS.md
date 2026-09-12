# Scarlet application artwork

Twelve original application covers made with the built-in image generation tool on 2026-09-11. Each desktop entry registers its cover using X-Scarlet-Background; the shared console card uses the accepted 16:9 image and reflected, blurred name panel.

The generated originals use a rounded 16:9 canvas of 1672×941. Packaging normalizes them to exactly 1280×720 PNG with `sips --resampleHeightWidth 720 1280`. Originals remain in the image tool's output directory; no manual painting or decorative effects were applied during packaging. Each cover is loaded through the same `ArtworkCache` as application-provided PNG/JPEG images. Blur and reflection are rendered by the shared console component, not baked into these assets.

## Files

Asset: `bundles/desktop/fs/share/app-art/files-cover.png`

```text
Use case: stylized-concept.
Asset type: original 16:9 library artwork for the Files application in Scarlet OS, an understated desktop operating system. Produce one full-bleed landscape 1536x864 image, not a UI mockup, no surrounding frame.
Subject: a carefully composed flat graphic still life of a large yellow file folder and a few overlapping pale paper sheets. The recognizable folder is warm yellow with an orange rear tab, no face, no brand mark. A few quieter slate-blue folder silhouettes create depth and rhythm across the width. A subtle architectural arrangement of file-like rectangular forms, deliberate and spacious.
Style: precise contemporary flat editorial illustration / silkscreen poster, crisp geometric shapes, extremely restrained fine grain, simple planar overlapping depth, visually mature and practical. Suitable as a coherent series of utility application covers, not a mobile app icon. Do not put the composition inside a square rounded icon tile.
Palette: solid dark graphite #252932 background, warm yellow #ffc800, ochre, slate grey, restrained cream. Use a small scarlet red #e04040 registration-like accent integrated into the composition. All lighting expressed as distinct flat shapes.
Composition: use the 16:9 landscape canvas intentionally. Main folder occupies about 50% of the height, centered slightly to the right, sufficient quiet surrounding space. Include meaningful warm yellow / ochre shapes in the lower quarter so a reflection below the artwork picks up the folder color.
Text: none, no letters, no numbers, no watermark.
Avoid: gradients, glossy 3D, glassmorphism, neon, glowing outlines, lens flare, dramatic cinematic light, fake UI, ornamental circuitry, sparkles, excessive objects, photographic textures. This is app cover artwork with restrained Scarlet OS character.
```

## terminal

Asset: `bundles/desktop/fs/share/app-art/terminal-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape utility application cover for Scarlet OS, ideally 1536x864. Input image is a style reference only: keep its carefully composed geometric editorial illustration, restrained paper-like surface, graphite background, and quiet practical character. Create a distinct composition for the requested subject; do not repeat the folder, papers, or yellow palette. No outer frame, no app icon tile, no UI mockup, no app name, no watermark. Use crisp large flat shapes, not gradients or glossy 3D. Avoid neon, glass, glow, cinematic lighting, decorative circuitry, futuristic HUDs and small unreadable details. Include subject-specific shapes and color in the lower quarter so a blurred reflection beneath this image carries recognizable local colors.
Subject: Terminal. A sophisticated graphic arrangement of a very large ivory command-prompt chevron and a scarlet red underscore cursor, with two staggered dark graphite terminal-panel planes behind it and a sparse rhythm of short ivory and slate code-line marks, no letters or numbers. The prompt symbols are built as graphic shapes, they should remain instantly legible at thumbnail size. Palette: graphite #20242b, slate #3f4b5b, pale ivory #e8e7df and scarlet #e04040. A restrained warm red flat strip in the lower-right composition. Strong 16:9 horizontal composition, no yellow.
```

## settings

Asset: `bundles/desktop/fs/share/app-art/settings-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape utility application cover for Scarlet OS, ideally 1536x864. Input image is a style reference only: keep its carefully composed geometric editorial illustration, restrained paper-like surface, graphite background, and quiet practical character. Create a distinct composition for the requested subject; do not repeat the folder, papers, or yellow palette. No outer frame, no app icon tile, no UI mockup, no app name, no watermark. Use crisp large flat shapes, not gradients or glossy 3D. Avoid neon, glass, glow, cinematic lighting, decorative circuitry, futuristic HUDs and small unreadable details. Include subject-specific shapes and color in the lower quarter so a blurred reflection beneath this image carries recognizable local colors.
Subject: Settings. A precise visual still life of two interlocking, broad-toothed gear wheels, one large off-white wheel, one smaller scarlet red wheel, and a quiet slate-blue circle or dial behind them. Technical but calm; these are flat engineered graphic shapes with distinct geometric shadow planes, not shiny hardware. The gears feel grounded and fill the center and lower half of a 16:9 landscape, with a spacious dark graphite upper area. Palette: graphite #252932, muted slate blue #637085, ivory #dfe3e5, scarlet #e04040. No yellow, no text, no random controls.
```

## clock

Asset: `bundles/desktop/fs/share/app-art/clock-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape utility application cover for Scarlet OS, ideally 1536x864. Input image is a style reference only: keep its carefully composed geometric editorial illustration, restrained paper-like surface, graphite background, and quiet practical character. Create a distinct composition for the requested subject; do not repeat the folder, papers, or yellow palette. No outer frame, no app icon tile, no UI mockup, no app name, no watermark. Use crisp large flat shapes, not gradients or glossy 3D. Avoid neon, glass, glow, cinematic lighting, decorative circuitry, futuristic HUDs and small unreadable details. Include subject-specific shapes and color in the lower quarter so a blurred reflection beneath this image carries recognizable local colors.
Subject: Clock. One large contemporary analog clock face, circular ivory dial with simple dark hour indices, restrained graphite hour and minute hands at approximately ten past ten, one fine scarlet red seconds hand, supported by two offset flat slate circular planes. A thin scarlet edge and warm rust-red flat foreground plane add Scarlet character. Landscape composition, the dial sits slightly right of center with clear negative space to the left; the lowest portion of the dial and its geometric shadow occupy the lower quarter. Palette: graphite #252932, ivory #e8e7df, slate #5e6877, rust #9a4439, scarlet #e04040. No numerals, no typography, no extra clocks, no yellow.
```



## Remaining applications

The following eight covers complete the 12 applications registered by the desktop bundle. Each used Files as the style reference; Boxcraft also used the existing Boxcraft screenshot fixture as a subject reference. Generation used the built-in imagegen tool. Asset packaging is the same exact 1280×720 normalization described above.

### Boxcraft

Asset: `bundles/desktop/fs/share/app-art/boxcraft-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape application cover for Scarlet OS, ideally 1536x864. The first input image is a STYLE REFERENCE: use the same mature geometric editorial illustration, carefully arranged large simple forms, restrained paper-like surface, flat planar shadows, solid dark graphite #252932 background, slate blue-grey supporting forms, ivory and a small Scarlet red #e04040 accent. Give this specific application its own subject and accent palette as described below. Do not copy the reference folder or paper silhouettes unless required by the subject. This is artwork, not a screenshot or an app icon: no outer frame, no rounded square tile, no window chrome, no app name or other text, no watermark, no third-party logos. Use the landscape intentionally, with the main subject across the center/lower half and enough breathing room above. The lower quarter must contain subject-specific shapes and color that will reflect into a separate name panel below the image. Keep the artwork sharp; the shell makes the reflection and blur separately. Avoid gradients, neon, glossy 3D, glass, glowing outlines, lens flares, ornamental circuitry, HUDs, sparkles, photorealism and lots of tiny details.
Subject: Boxcraft, a voxel world building game. The SECOND input is CONTENT REFERENCE ONLY: grass-topped earth cubes, a terraced block landscape and cubic trees. Reimagine that world as an elegant geometric graphic diorama in the style of the first image, not as a screenshot. Several broad green grass blocks at staggered heights, warm ochre soil faces, one or two blocky leafy trees and a simple sandy path. Small flat blue water inset is welcome. A solid graphite upper background and sparse slate silhouettes keep it part of the same Scarlet cover family. View the whole compact landscape from a slightly raised angle; generous foreground earth/grass planes continue to the bottom edge. Palette: moss and grass green, ochre, warm terracotta, graphite, restrained cyan and a tiny scarlet marker. No folders, no game HUD, no crosshair, no letters.
```

### Myrica

Asset: `bundles/desktop/fs/share/app-art/myrica-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape application cover for Scarlet OS, ideally 1536x864. The first input image is a STYLE REFERENCE: use the same mature geometric editorial illustration, carefully arranged large simple forms, restrained paper-like surface, flat planar shadows, solid dark graphite #252932 background, slate blue-grey supporting forms, ivory and a small Scarlet red #e04040 accent. Give this specific application its own subject and accent palette as described below. Do not copy the reference folder or paper silhouettes unless required by the subject. This is artwork, not a screenshot or an app icon: no outer frame, no rounded square tile, no window chrome, no app name or other text, no watermark, no third-party logos. Use the landscape intentionally, with the main subject across the center/lower half and enough breathing room above. The lower quarter must contain subject-specific shapes and color that will reflect into a separate name panel below the image. Keep the artwork sharp; the shell makes the reflection and blur separately. Avoid gradients, neon, glossy 3D, glass, glowing outlines, lens flares, ornamental circuitry, HUDs, sparkles, photorealism and lots of tiny details.
Subject: Myrica, the web browser. A large ivory globe with broad graphite longitude/latitude curves, an understated orbit arc, and two offset flat slate/teal page-like panels behind it. A small scarlet rectangular tab and a simple directional arrow accent suggest navigation. Grounded, precise graphic still life; no brand logo and no search text. The globe is toward the right half, the orbit and panels extend horizontally to use 16:9. Palette: graphite, ivory, muted teal, cobalt blue and one small scarlet accent; blue/teal geometric foreground. No folder motifs.
```

### Notepad

Asset: `bundles/desktop/fs/share/app-art/notepad-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape application cover for Scarlet OS, ideally 1536x864. The first input image is a STYLE REFERENCE: use the same mature geometric editorial illustration, carefully arranged large simple forms, restrained paper-like surface, flat planar shadows, solid dark graphite #252932 background, slate blue-grey supporting forms, ivory and a small Scarlet red #e04040 accent. Give this specific application its own subject and accent palette as described below. Do not copy the reference folder or paper silhouettes unless required by the subject. This is artwork, not a screenshot or an app icon: no outer frame, no rounded square tile, no window chrome, no app name or other text, no watermark, no third-party logos. Use the landscape intentionally, with the main subject across the center/lower half and enough breathing room above. The lower quarter must contain subject-specific shapes and color that will reflect into a separate name panel below the image. Keep the artwork sharp; the shell makes the reflection and blur separately. Avoid gradients, neon, glossy 3D, glass, glowing outlines, lens flares, ornamental circuitry, HUDs, sparkles, photorealism and lots of tiny details.
Subject: Notepad, the plain text editor. Two broad ivory paper sheets slightly offset, only four quiet slate horizontal writing marks, and a carefully simplified teal pencil placed diagonally across them. A small scarlet bookmark/tab and pale green geometric foreground ground the scene. Keep it a precise flat graphic still life, no written words, no notebook spiral, no folder, no monitor or fake application screenshot. Palette: graphite, ivory, muted emerald/teal, pale grey, tiny scarlet accent. Main paper-and-pencil composition spans the center and lower half.
```

### Media Player

Asset: `bundles/desktop/fs/share/app-art/media-player-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape application cover for Scarlet OS, ideally 1536x864. The first input image is a STYLE REFERENCE: use the same mature geometric editorial illustration, carefully arranged large simple forms, restrained paper-like surface, flat planar shadows, solid dark graphite #252932 background, slate blue-grey supporting forms, ivory and a small Scarlet red #e04040 accent. Give this specific application its own subject and accent palette as described below. Do not copy the reference folder or paper silhouettes unless required by the subject. This is artwork, not a screenshot or an app icon: no outer frame, no rounded square tile, no window chrome, no app name or other text, no watermark, no third-party logos. Use the landscape intentionally, with the main subject across the center/lower half and enough breathing room above. The lower quarter must contain subject-specific shapes and color that will reflect into a separate name panel below the image. Keep the artwork sharp; the shell makes the reflection and blur separately. Avoid gradients, neon, glossy 3D, glass, glowing outlines, lens flares, ornamental circuitry, HUDs, sparkles, photorealism and lots of tiny details.
Subject: Media Player, the audio/music player. A large flat charcoal vinyl record with an ivory and muted plum center label, a clearly readable ivory pair of musical notes, and five broad quiet waveform bars behind it. Use crisp planar geometry and shadow shapes, no shiny reflections. A plum/magenta geometric foreground continues to the lower edge. Palette: graphite, slate, muted plum #795478, warm ivory, small scarlet accent. A mature audio artwork composition with horizontal breathing room; no album text, headphones, folders or third-party marks.
```

### Vellum

Asset: `bundles/desktop/fs/share/app-art/vellum-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape application cover for Scarlet OS, ideally 1536x864. The first input image is a STYLE REFERENCE: use the same mature geometric editorial illustration, carefully arranged large simple forms, restrained paper-like surface, flat planar shadows, solid dark graphite #252932 background, slate blue-grey supporting forms, ivory and a small Scarlet red #e04040 accent. Give this specific application its own subject and accent palette as described below. Do not copy the reference folder or paper silhouettes unless required by the subject. This is artwork, not a screenshot or an app icon: no outer frame, no rounded square tile, no window chrome, no app name or other text, no watermark, no third-party logos. Use the landscape intentionally, with the main subject across the center/lower half and enough breathing room above. The lower quarter must contain subject-specific shapes and color that will reflect into a separate name panel below the image. Keep the artwork sharp; the shell makes the reflection and blur separately. Avoid gradients, neon, glossy 3D, glass, glowing outlines, lens flares, ornamental circuitry, HUDs, sparkles, photorealism and lots of tiny details.
Subject: Vellum, the image and PDF document viewer. Two slightly overlapping ivory print/document sheets: the front one contains a bold flat landscape image, broad sage and slate mountain silhouettes under a small warm coral sun; the back sheet shows only a few restrained horizontal graphic lines. A small magenta page-corner accent. The sheets are large enough to read as viewing images and documents at thumbnail scale, with a quiet mauve/slate geometric foreground. Palette: graphite, ivory, muted sage, dusty mauve/magenta and coral red. No pencil, no folder, no camera, no readable text.
```

### Video Player

Asset: `bundles/desktop/fs/share/app-art/video-player-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape application cover for Scarlet OS, ideally 1536x864. The first input image is a STYLE REFERENCE: use the same mature geometric editorial illustration, carefully arranged large simple forms, restrained paper-like surface, flat planar shadows, solid dark graphite #252932 background, slate blue-grey supporting forms, ivory and a small Scarlet red #e04040 accent. Give this specific application its own subject and accent palette as described below. Do not copy the reference folder or paper silhouettes unless required by the subject. This is artwork, not a screenshot or an app icon: no outer frame, no rounded square tile, no window chrome, no app name or other text, no watermark, no third-party logos. Use the landscape intentionally, with the main subject across the center/lower half and enough breathing room above. The lower quarter must contain subject-specific shapes and color that will reflect into a separate name panel below the image. Keep the artwork sharp; the shell makes the reflection and blur separately. Avoid gradients, neon, glossy 3D, glass, glowing outlines, lens flares, ornamental circuitry, HUDs, sparkles, photorealism and lots of tiny details.
Subject: Video Player. A broad dark filmstrip, angled gently through the horizontal composition, with three large ivory/slate rectangular frame openings and a bold scarlet red play triangle in the main central frame. One quiet slate-purple rectangular panel behind it. Suggest moving pictures with the filmstrip shape and one single triangle, not with a fake screenshot. Ground with muted purple and graphite geometric foreground planes. Palette: graphite, ivory, muted purple #746489, slate and scarlet. No vinyl record, no music notes, no text, no folder silhouettes.
```

### Task Manager

Asset: `bundles/desktop/fs/share/app-art/task-manager-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape application cover for Scarlet OS, ideally 1536x864. The first input image is a STYLE REFERENCE: use the same mature geometric editorial illustration, carefully arranged large simple forms, restrained paper-like surface, flat planar shadows, solid dark graphite #252932 background, slate blue-grey supporting forms, ivory and a small Scarlet red #e04040 accent. Give this specific application its own subject and accent palette as described below. Do not copy the reference folder or paper silhouettes unless required by the subject. This is artwork, not a screenshot or an app icon: no outer frame, no rounded square tile, no window chrome, no app name or other text, no watermark, no third-party logos. Use the landscape intentionally, with the main subject across the center/lower half and enough breathing room above. The lower quarter must contain subject-specific shapes and color that will reflect into a separate name panel below the image. Keep the artwork sharp; the shell makes the reflection and blur separately. Avoid gradients, neon, glossy 3D, glass, glowing outlines, lens flares, ornamental circuitry, HUDs, sparkles, photorealism and lots of tiny details.
Subject: Task Manager, the process and resource monitor. A composed cluster of broad vertical cyan/teal bar-chart slabs at different heights, one continuous ivory performance trace crossing in front, and a quiet half-circle gauge form behind the bars. These are clean physical-looking flat graphic planes, not a screenful of tiny UI widgets. Main cluster slightly right of center, teal geometric foreground across the bottom, a small scarlet peak/cursor accent. Palette: graphite, cool slate, muted cyan/teal #3995a1, ivory, tiny scarlet. No numbers, no words, no circuitry, no folders.
```

### Widget Factory

Asset: `bundles/desktop/fs/share/app-art/widget-factory-cover.png`

```text
Use case: stylized-concept. Create one original full-bleed 16:9 landscape application cover for Scarlet OS, ideally 1536x864. The first input image is a STYLE REFERENCE: use the same mature geometric editorial illustration, carefully arranged large simple forms, restrained paper-like surface, flat planar shadows, solid dark graphite #252932 background, slate blue-grey supporting forms, ivory and a small Scarlet red #e04040 accent. Give this specific application its own subject and accent palette as described below. Do not copy the reference folder or paper silhouettes unless required by the subject. This is artwork, not a screenshot or an app icon: no outer frame, no rounded square tile, no window chrome, no app name or other text, no watermark, no third-party logos. Use the landscape intentionally, with the main subject across the center/lower half and enough breathing room above. The lower quarter must contain subject-specific shapes and color that will reflect into a separate name panel below the image. Keep the artwork sharp; the shell makes the reflection and blur separately. Avoid gradients, neon, glossy 3D, glass, glowing outlines, lens flares, ornamental circuitry, HUDs, sparkles, photorealism and lots of tiny details.
Subject: Widget Factory, a developer's showcase of UI components. A deliberate geometric still life built from a few large interface component shapes: one broad ivory rounded button slab, a slate toggle with a scarlet circular thumb, a single blue horizontal slider with an ivory knob, and a small stack of overlapping slate panels. Arrange these parts as an elegant toolkit assembly, with a little offset depth and flat shadow planes. No actual application screenshot, no text, no interface chrome. Palette: graphite, slate, ivory, muted cobalt blue and scarlet red. Blue-grey geometric foreground. No gears, folders, monitors or tiny decorative widgets.
```

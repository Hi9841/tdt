# Acquired asset provenance

Downloaded 2026-09-21. Photographer: Kaboompics. Pexels license: https://www.pexels.com/license/ permits website use and cropping; do not imply endorsement. These are illustrative scenes, not TDT users or product captures.

- `hero-mobile-source.jpg`: https://www.pexels.com/photo/woman-with-headphones-and-microphone-using-laptop-5399042/ . Downloaded at width 1600. `hero-dictation-mobile.jpg` is a 960x2400 crop at (0,0). Use the approved opaque bottom caption surface.
- `notes-source.jpg`: https://www.pexels.com/photo/photo-of-a-woman-using-headphones-and-microphone-5399043/ . Downloaded at width 2000. `notes-dictation.jpg` is a 1974x1050 crop at (0,650). Face is in the right half; captions remain separate.
- `reply-source.jpg`: https://www.pexels.com/photo/woman-working-on-a-laptop-while-sitting-behind-a-glass-desk-wearing-headphones-and-using-a-microphone-5399036/ . Downloaded at width 2000. `reply-dictation.jpg` is a 1974x1050 candidate crop at (0,450), rejected for the approved composition because the face falls under the left-half refraction. Not approved for Build.
- `tdt-brand-icon.png`: unchanged copy of project `branding/tdt-app-icon.png`, 1024x1024. Preserve original colors and proportions.

Existing local font families and OFL notices retained in `../fonts/figtree/` and `../fonts/cormorant-garamond/`. Family names were read from the actual font files. Official family repositories checked: https://github.com/google/fonts/tree/main/ofl/figtree and https://github.com/google/fonts/tree/main/ofl/cormorantgaramond . Browser loading is a Build check.

No image-generation tool was available. Desktop hero remains unresolved because a matching sparse still-life composition was not acquired. Existing state captures are below minimum raster dimensions. Settings capture freshness remains unverified. Do not use enlarged old captures as replacements.

## Revised blueprint acquisition, 2026-09-21

The revised page plan supersedes the unresolved desktop still-life requirement above and removes all screenshot needs in favor of a native instructional component.

- `hero-dictation-desktop.jpg`: JPEG 1806x1254, ratio 301:209, cropped from unchanged `notes-source.jpg` at (0,550). Source: https://www.pexels.com/photo/photo-of-a-woman-using-headphones-and-microphone-5399043/ . Kaboompics, Pexels license. Inspected: face right of center, pale left reading area, microphone retained. Keep bottom headline below the face and use the approved opaque copy surface.
- `reply-dictation-approved.jpg`: JPEG 1974x1390, ratio 987:695, cropped from unchanged `notes-source.jpg` at (0,700). Same source and license. Inspected: face in right half, microphone and laptop visible. Use top-aligned desktop cropping to 47:25; compact frame approximately 71:50. Keep refraction confined to the left half and captions below the face.
- The two use stories intentionally illustrate different writing possibilities using one photographed session. Neither establishes actual TDT usage, a customer, or endorsement. Preserve the visible Illustrative uses label.
- The native Windows workflow illustration remains for Build, with representative example text. No screenshot is acquired or required by the revised plan.

## Current Page asset reconciliation, 2026-09-21

The current blueprint uses Mello's tall-left and wide-right photo composition. This supersedes the earlier Avoora refraction instructions for these images.

- `notes_dictation_photo`: primary destination `notes-dictation-tall.jpg`, real JPEG 1392x1600, ratio 87:100. Cropped without enlargement from retained `notes-source.jpg` at x400, y550, width1392, height1600 using System.Drawing. Source: https://www.pexels.com/photo/photo-of-a-woman-using-headphones-and-microphone-5399043/ . Kaboompics, existing Pexels license. Inspected at full crop: complete face and headphones, useful microphone and laptop foreground. Keep opaque caption in the lower 20%; no refraction. Preserve `notes-dictation.jpg` (1974x1050) as the compact source, cover to 71:50 at horizontal position 65%, vertical center; face and microphone remain visible. Both are derivatives of the same licensed session.
- `reply_dictation_photo`: retain `reply-dictation-approved.jpg` 1974x1390. Expanded 161:100 frame crops vertically from the top; compact 71:50 nearly matches the file. Captions stay below the face; no refraction.
- Hero JPEGs and the original PNG icon were decoded successfully. Original and acquired logo SHA256 both equal DA1B6B931B29DE6DA1C1BD5968041FAD7CA1EE9779C1FCEEF02560B8E5646C08.
- All selected rasters meet the size and pixel limits. Font files were accepted by the local font loader and family names confirmed. Both local OFL notices remain present; official Google Fonts family repositories were checked again. Existing Figtree normal/italic variable files and Cormorant 500/600 normal/italic files require no download. Extra existing static weights remain preserved, but do not authorize new typographic choices.
- Asset inspection does not establish browser layout, font loading, motion or accessibility completion. No page implementation files or dependencies were changed.

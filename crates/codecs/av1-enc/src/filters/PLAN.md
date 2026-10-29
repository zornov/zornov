# Step 2 plan: in-loop filters

Bit-exactness: the encoder runs the decoder's own `studio_av1::PostFilter` (deblock, CDEF,
restoration) on its pre-filter reconstruction, so reference frames equal the decoder's output.

Order, each with its own bit-exact check on studio-av1 and VideoToolbox, and bytes at equal PSNR
against filters-off:
1. Deblock: lf levels per frame chosen by a small search (PSNR of the filtered recon), sharpness 0.
2. CDEF: strength set per frame, `cdef_idx` per 64x64 chosen by search.
3. Loop restoration: Wiener and self-guided per unit, chosen by SSE.
Tile data does not depend on the filter parameters, so the frame header is written after the search.

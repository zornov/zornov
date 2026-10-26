# Models used by `studio-ai`

Every model is openly licensed and is downloaded only when the user asks
(Settings > AI models, or `cargo run -p studio-ai --example models --
download <id>`), from the upstream location below, into
`~/Movies/Studio/Models/<id>/`. Each file is checked against the SHA-256 in
[`crates/ai/ai/src/registry.rs`](../crates/ai/src/registry.rs) before it is used; URLs are pinned to a
fixed revision so the checksums stay valid. Nothing is bundled with Studio.

| id | Model | Used for | Licence | Size | Upstream project | Files downloaded from |
|---|---|---|---|---|---|---|
| `whisper-tiny` | Whisper tiny (multilingual) | Captions, filler words | MIT | 154 MB | [openai/whisper](https://github.com/openai/whisper) | ONNX export [onnx-community/whisper-tiny_timestamped](https://huggingface.co/onnx-community/whisper-tiny_timestamped) @ `5172442` |
| `whisper-base` | Whisper base (multilingual) | Captions (better) | MIT | 293 MB | [openai/whisper](https://github.com/openai/whisper) | [onnx-community/whisper-base_timestamped](https://huggingface.co/onnx-community/whisper-base_timestamped) @ `608c49e` |
| `whisper-small` | Whisper small (multilingual) | Captions (best, slower) | MIT | 970 MB | [openai/whisper](https://github.com/openai/whisper) | [onnx-community/whisper-small_timestamped](https://huggingface.co/onnx-community/whisper-small_timestamped) @ `65caa70` |
| `modnet` | MODNet portrait matting | Remove background | Apache-2.0 | 26 MB | [ZHKKKe/MODNet](https://github.com/ZHKKKe/MODNet) | [Xenova/modnet](https://huggingface.co/Xenova/modnet) @ `fa2fa54` |
| `sam2.1-tiny` | SAM 2.1 Hiera tiny | Object cut-out | Apache-2.0 | 156 MB | [facebookresearch/sam2](https://github.com/facebookresearch/sam2) | [onnx-community/sam2.1-hiera-tiny-ONNX](https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX) @ `814a066` |
| `deepfilternet3` | DeepFilterNet 3 | Voice cleanup | MIT OR Apache-2.0 | 8 MB | [Rikorose/DeepFilterNet](https://github.com/Rikorose/DeepFilterNet) | `models/DeepFilterNet3_onnx.tar.gz` @ `84d57ec` (unpacked to `enc.onnx`, `erb_dec.onnx`, `df_dec.onnx`) |
| `rife-4.9` | RIFE 4.9 (Practical-RIFE) | Smooth slow motion | MIT | 21 MB | [hzwer/Practical-RIFE](https://github.com/hzwer/Practical-RIFE) | ONNX export [yuvraj108c/rife-onnx](https://huggingface.co/yuvraj108c/rife-onnx) @ `64de726` (`rife49_ensemble_True_scale_1_sim.onnx`) |
| `depth-anything-v2-small` | Depth Anything V2 small | Depth effects | Apache-2.0 | 99 MB | [DepthAnything/Depth-Anything-V2](https://github.com/DepthAnything/Depth-Anything-V2) | [onnx-community/depth-anything-v2-small](https://huggingface.co/onnx-community/depth-anything-v2-small) @ `4472b73` |
| `face-detector` | BlazeFace short range | Face tracking, auto reframe | Apache-2.0 | 230 kB | [MediaPipe face detector](https://ai.google.dev/edge/mediapipe/solutions/vision/face_detector) | `storage.googleapis.com/mediapipe-models/face_detector/blaze_face_short_range/float16/1/` |
| `pose-landmarker-lite` | BlazePose landmarker lite | Body tracking | Apache-2.0 | 6 MB | [MediaPipe pose landmarker](https://ai.google.dev/edge/mediapipe/solutions/vision/pose_landmarker) | `storage.googleapis.com/mediapipe-models/pose_landmarker/pose_landmarker_lite/float16/1/` |
| `realesr-general-x4v3` | Real-ESRGAN general x4v3 (SRVGGNetCompact) | Upscale and enhance (super-resolution, denoise, deblock, restoration) | BSD-3-Clause | 5 MB | [xinntao/Real-ESRGAN](https://github.com/xinntao/Real-ESRGAN) | ONNX export of the official `realesr-general-x4v3.pth` [vlad-grig/realesr-general-x4v3-onnx](https://huggingface.co/vlad-grig/realesr-general-x4v3-onnx) @ `0e95d93` (`onnx/model.onnx`) |
| `upernet-convnext-tiny` | UperNet + ConvNeXt-T, ADE20K scene parsing | Scene regions (sky, water, trees... masks) | MIT | 238 MB | [openmmlab/upernet-convnext-tiny](https://huggingface.co/openmmlab/upernet-convnext-tiny) (UperNet, Xiao et al.; ConvNeXt, Liu et al.) | ONNX export, unmodified fp32 weights, [JElvis/upernet-convnext-tiny-ade-onnx](https://huggingface.co/JElvis/upernet-convnext-tiny-ade-onnx) @ `a8e2807` |
| `owlv2-base-ensemble` | OWLv2 base patch 16 ensemble (open-vocabulary detector), fp16 | Object map: finds anything that can be named, from text | Apache-2.0 | 311 MB | [google-research/scenic OWL-ViT](https://github.com/google-research/scenic/tree/main/scenic/projects/owl_vit), checkpoint [google/owlv2-base-patch16-ensemble](https://huggingface.co/google/owlv2-base-patch16-ensemble) | ONNX export [onnx-community/owlv2-base-patch16-ensemble-ONNX](https://huggingface.co/onnx-community/owlv2-base-patch16-ensemble-ONNX) @ `180d6ef` (`onnx/model_fp16.onnx`, `tokenizer.json`) |
| `siglip-base-224` | SigLIP base patch 16, 224 px (picture-text embeddings), fp16 | Find a moment by description; name a crop | Apache-2.0 | 409 MB | [google-research/big_vision](https://github.com/google-research/big_vision), checkpoint [google/siglip-base-patch16-224](https://huggingface.co/google/siglip-base-patch16-224) | ONNX export [Xenova/siglip-base-patch16-224](https://huggingface.co/Xenova/siglip-base-patch16-224) @ `4649052` (`onnx/vision_model_fp16.onnx`, `onnx/text_model_fp16.onnx`, `tokenizer.json`) |
| `paddleocr-latin` | PaddleOCR PP-OCRv3 text detector + PP-OCRv5 Latin text recogniser | Read text in the picture (signs, labels, screens) | Apache-2.0 | 10 MB | [PaddlePaddle/PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) | ONNX export [monkt/paddleocr-onnx](https://huggingface.co/monkt/paddleocr-onnx) @ `7b02d0a` (`detection/v3/det.onnx`, `languages/latin/rec.onnx`, `languages/latin/dict.txt`) |
| `triposr` | TripoSR (single picture to 3D model; triplane transformer and field network) | Picture to 3D model (`generate` task `model3d`) | MIT (code and weights, Tripo AI and Stability AI) | 1.7 GB | [VAST-AI-Research/TripoSR](https://github.com/VAST-AI-Research/TripoSR), weights [stabilityai/TripoSR](https://huggingface.co/stabilityai/TripoSR) | ONNX export of the official weights, fp32, [fernandotonon/QtMeshEditor-triposr-onnx](https://huggingface.co/fernandotonon/QtMeshEditor-triposr-onnx) @ `01b2f1a` (`triposr_encoder.onnx`, `triposr_decoder.onnx`) |

Notes

- Scene regions use UperNet/ConvNeXt (MIT) rather than SegFormer, whose
  weights carry NVIDIA's non-commercial licence.
- Only the *small* Depth Anything V2 model is Apache-2.0 (the larger ones
  are non-commercial), so only the small one is listed.
- Video matting models under copyleft or non-commercial licences (for
  example RVM, GPL-3.0) are deliberately not used; MODNet plus temporal
  smoothing does that job.
- Scene cuts, saliency for auto reframe, silence detection, beat detection
  and the voice-cleanup / slow-motion fallbacks use no model at all.
- The ONNX exports on Hugging Face are conversions of the upstream weights
  and carry the upstream licence.
- The object map uses OWLv2 rather than Grounding DINO, whose licence
  chain (and that of the weights most exports carry) is less clear than
  Google's Apache-2.0 release of OWL-ViT v2. The 307 MB fp16 export is
  used; the 614 MB fp32 export gives the same boxes.
- Picture-text matching uses SigLIP (Apache-2.0, stated on the model
  card) rather than OpenAI's CLIP, whose Hugging Face card carries no
  licence field and asks for research use only.
- Text reading is Latin script only (English and about thirty other
  languages); PaddleOCR's Chinese, Korean, Arabic and other recognisers
  are available in the same export repository under the same licence.
- The tokenizers (a byte-level BPE for OWLv2, a SentencePiece unigram for
  SigLIP) are read from each model's own `tokenizer.json`.

Credits (for the About box): OpenAI Whisper; MODNet (Ke et al.); SAM 2
(Meta AI); DeepFilterNet (Schröter et al.); RIFE (Huang et al.); Depth
Anything V2 (Yang et al.); MediaPipe BlazeFace and BlazePose (Google); Real-ESRGAN (Wang et al., BSD-3-Clause); OWLv2 (Minderer et al., Google); SigLIP (Zhai et al., Google); PaddleOCR (PaddlePaddle authors); UperNet (Xiao et al.) with ConvNeXt (Liu et al.), OpenMMLab weights., Meta FAIR).

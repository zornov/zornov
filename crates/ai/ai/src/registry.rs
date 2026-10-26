// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The model registry: every model a feature can use, where it comes from,
//! its licence and the SHA-256 of every file.
//!
//! Each model lives in its own folder `<models root>/<ModelId::dir>/`. URLs
//! point at a fixed upstream revision (a commit hash or a versioned bucket
//! path), so the checksums below stay valid. `docs/models.md` lists
//! the same information for people.

/// Every model the AI features know about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelId {
    WhisperTiny,
    WhisperBase,
    WhisperSmall,
    Modnet,
    Sam21Tiny,
    DeepFilterNet3,
    Rife49,
    DepthAnythingV2Small,
    FaceDetector,
    PoseLandmarker,
    RealEsrganGeneralX4v3,
    UperNetConvNextTiny,
    OwlV2Base,
    SiglipBase224,
    PaddleOcrLatin,
    TripoSr,
}

/// How a downloaded file becomes model files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unpack {
    /// The download is the model file itself.
    None,
    /// A `.tar.gz`; `(member path in archive, destination name, sha256, size)`.
    TarGz(&'static [(&'static str, &'static str, &'static str, u64)]),
}

/// One file to download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSpec {
    pub url: &'static str,
    /// Name inside the model folder (for archives: the archive's name; it is
    /// deleted after unpacking).
    pub name: &'static str,
    /// Lower-case hex SHA-256 of the downloaded bytes.
    pub sha256: &'static str,
    pub size: u64,
    pub unpack: Unpack,
}

impl FileSpec {
    /// Names (and expected hashes/sizes) of the files this download leaves in
    /// the model folder.
    pub fn installed_files(&self) -> Vec<(&'static str, &'static str, u64)> {
        match self.unpack {
            Unpack::None => vec![(self.name, self.sha256, self.size)],
            Unpack::TarGz(members) => members.iter().map(|&(_, dest, sha, size)| (dest, sha, size)).collect(),
        }
    }
}

/// Everything known about a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    pub id: ModelId,
    /// Folder name under the models root.
    pub dir: &'static str,
    /// Display name.
    pub name: &'static str,
    /// The feature that needs it, for messages ("Captions needs ...").
    pub used_for: &'static str,
    /// SPDX licence identifier.
    pub licence: &'static str,
    /// Upstream project page (for the credits / docs/models.md).
    pub source: &'static str,
    pub files: &'static [FileSpec],
}

impl ModelSpec {
    /// Total bytes to download.
    pub fn download_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// `12.3 MB`-style size for messages.
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} kB", (b / 1e3).max(1.0))
    }
}

macro_rules! hf {
    ($repo:literal, $rev:literal, $path:literal) => {
        concat!("https://huggingface.co/", $repo, "/resolve/", $rev, "/", $path)
    };
}

const WHISPER_TINY: &[FileSpec] = &[
    FileSpec { url: hf!("onnx-community/whisper-tiny_timestamped", "517244293732ee2d58139af5814231b7e6830a0d", "onnx/encoder_model.onnx"), name: "encoder_model.onnx", sha256: "8944ec04111e040305cdb07ac97f50363024255bf0bdc67ab93ecb76645f31ae", size: 32_894_434, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-tiny_timestamped", "517244293732ee2d58139af5814231b7e6830a0d", "onnx/decoder_model.onnx"), name: "decoder_model.onnx", sha256: "6f9271b9f93784cab472008e8336d6811ccf117dc1477b88b7458d62b8adae5d", size: 118_423_329, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-tiny_timestamped", "517244293732ee2d58139af5814231b7e6830a0d", "tokenizer.json"), name: "tokenizer.json", sha256: "27fc476bfe7f17299480be2273fc0608e4d5a99aba2ab5dec5374b4482d1a566", size: 2_480_466, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-tiny_timestamped", "517244293732ee2d58139af5814231b7e6830a0d", "generation_config.json"), name: "generation_config.json", sha256: "f5c67e5a4f7102f8cb4d058bc95da276bbc19eeec997267c3bb0f25ef68facd1", size: 3_772, unpack: Unpack::None },
];

const WHISPER_BASE: &[FileSpec] = &[
    FileSpec { url: hf!("onnx-community/whisper-base_timestamped", "608c49e61301901684bc36cac8f74b95ff6b5a8e", "onnx/encoder_model.onnx"), name: "encoder_model.onnx", sha256: "7fcea817bb2be4d86729b521e5a7fcbec28fa743edfed67e882b33ff15852540", size: 82_451_730, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-base_timestamped", "608c49e61301901684bc36cac8f74b95ff6b5a8e", "onnx/decoder_model.onnx"), name: "decoder_model.onnx", sha256: "21307ff74dc502fe5a4aacf80f2d316fc67976b5ee5810cc209ab85a9a443abd", size: 208_328_538, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-base_timestamped", "608c49e61301901684bc36cac8f74b95ff6b5a8e", "tokenizer.json"), name: "tokenizer.json", sha256: "27fc476bfe7f17299480be2273fc0608e4d5a99aba2ab5dec5374b4482d1a566", size: 2_480_466, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-base_timestamped", "608c49e61301901684bc36cac8f74b95ff6b5a8e", "generation_config.json"), name: "generation_config.json", sha256: "61070cf8de25b1e9256e8e102ded49d8d24a8369ed36ef84fdf21549e68125a0", size: 3_832, unpack: Unpack::None },
];

const WHISPER_SMALL: &[FileSpec] = &[
    FileSpec { url: hf!("onnx-community/whisper-small_timestamped", "65caa70f294b46e1c33ff820aae6b16d048ab818", "onnx/encoder_model.onnx"), name: "encoder_model.onnx", sha256: "c0908c1ef2326f5487d1f77e56c6925f910ae56c87fa269ed885682ac851e588", size: 352_791_798, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-small_timestamped", "65caa70f294b46e1c33ff820aae6b16d048ab818", "onnx/decoder_model.onnx"), name: "decoder_model.onnx", sha256: "07472ac2d328746ad674fedd553d4c6e730736ae29922b48d7645645d8faaa28", size: 614_942_826, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-small_timestamped", "65caa70f294b46e1c33ff820aae6b16d048ab818", "tokenizer.json"), name: "tokenizer.json", sha256: "27fc476bfe7f17299480be2273fc0608e4d5a99aba2ab5dec5374b4482d1a566", size: 2_480_466, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/whisper-small_timestamped", "65caa70f294b46e1c33ff820aae6b16d048ab818", "generation_config.json"), name: "generation_config.json", sha256: "f538b28220c6a6d6f1af1458d4141cacb4ef4963df3de98a19490440c412ddf0", size: 3_893, unpack: Unpack::None },
];

const MODNET: &[FileSpec] = &[FileSpec { url: hf!("Xenova/modnet", "fa2fa546052fba4c08921230a26cc69a333fca12", "onnx/model.onnx"), name: "model.onnx", sha256: "07c308cf0fc7e6e8b2065a12ed7fc07e1de8febb7dc7839d7b7f15dd66584df9", size: 25_888_640, unpack: Unpack::None }];

const SAM21_TINY: &[FileSpec] = &[
    FileSpec { url: hf!("onnx-community/sam2.1-hiera-tiny-ONNX", "814a066640debee5a91e70aa401fb8e17e030503", "onnx/vision_encoder.onnx"), name: "vision_encoder.onnx", sha256: "4f30aacd3aaefbca81a0b7fe4c1fc96345570ea0a6f80ced599493d1b3be2e8c", size: 354_238, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/sam2.1-hiera-tiny-ONNX", "814a066640debee5a91e70aa401fb8e17e030503", "onnx/vision_encoder.onnx_data"), name: "vision_encoder.onnx_data", sha256: "e83df9866a5afe68ea7f0f721f18f65137fc3acbf0da1c74e946d363e09c69cc", size: 134_084_864, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/sam2.1-hiera-tiny-ONNX", "814a066640debee5a91e70aa401fb8e17e030503", "onnx/prompt_encoder_mask_decoder.onnx"), name: "prompt_encoder_mask_decoder.onnx", sha256: "874414704c5d686db7d206a35f6e15d26563d50c8c4468fccc6739bd7e491dcf", size: 213_114, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/sam2.1-hiera-tiny-ONNX", "814a066640debee5a91e70aa401fb8e17e030503", "onnx/prompt_encoder_mask_decoder.onnx_data"), name: "prompt_encoder_mask_decoder.onnx_data", sha256: "e9874d900dd4134ed60eab1e97910327c2419e0b2954485d8fd6e7f1a1470f47", size: 20_958_208, unpack: Unpack::None },
];

const DFN3: &[FileSpec] = &[FileSpec {
    url: "https://github.com/Rikorose/DeepFilterNet/raw/84d57ec2c08fe08e68a13fb32a58cd7092060a0f/models/DeepFilterNet3_onnx.tar.gz",
    name: "DeepFilterNet3_onnx.tar.gz",
    sha256: "c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616",
    size: 7_983_136,
    unpack: Unpack::TarGz(&[
        ("tmp/export/enc.onnx", "enc.onnx", "7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916", 1_954_042),
        ("tmp/export/erb_dec.onnx", "erb_dec.onnx", "ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895", 3_292_397),
        ("tmp/export/df_dec.onnx", "df_dec.onnx", "23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a", 3_340_803),
    ]),
}];

const RIFE49: &[FileSpec] = &[FileSpec { url: hf!("yuvraj108c/rife-onnx", "64de7265b6a06637c2f2c6a92ecd1972326499fe", "rife49_ensemble_True_scale_1_sim.onnx"), name: "rife49.onnx", sha256: "76e4cef9ab42fa7dd4e8f6e4aba47462051e3faa969e4bca6479784fbab0ac6f", size: 21_458_882, unpack: Unpack::None }];

const DEPTH: &[FileSpec] = &[FileSpec { url: hf!("onnx-community/depth-anything-v2-small", "4472b7362082ad9968fee890ca0f1e5aca36b93d", "onnx/model.onnx"), name: "model.onnx", sha256: "afb6a5c28f3b6bf1618c6e43f02073ef9dfdc70e937502d51603e57b0a1df10c", size: 99_060_839, unpack: Unpack::None }];

const FACE: &[FileSpec] = &[FileSpec { url: "https://storage.googleapis.com/mediapipe-models/face_detector/blaze_face_short_range/float16/1/blaze_face_short_range.tflite", name: "blaze_face_short_range.tflite", sha256: "b4578f35940bf5a1a655214a1cce5cab13eba73c1297cd78e1a04c2380b0152f", size: 229_746, unpack: Unpack::None }];

const POSE: &[FileSpec] = &[FileSpec { url: "https://storage.googleapis.com/mediapipe-models/pose_landmarker/pose_landmarker_lite/float16/1/pose_landmarker_lite.task", name: "pose_landmarker_lite.task", sha256: "59929e1d1ee95287735ddd833b19cf4ac46d29bc7afddbbf6753c459690d574a", size: 5_777_746, unpack: Unpack::None }];

const REALESR: &[FileSpec] = &[FileSpec { url: hf!("vlad-grig/realesr-general-x4v3-onnx", "0e95d93e454cf6b5de9bdf5a1c9e7ec340d38231", "onnx/model.onnx"), name: "model.onnx", sha256: "7b10e80021ec3e0ff35017ae5022791879d93a799b2e201e1010a37510202266", size: 4_866_420, unpack: Unpack::None }];

const UPERNET: &[FileSpec] = &[FileSpec { url: hf!("JElvis/upernet-convnext-tiny-ade-onnx", "a8e2807c3479bc617b37d8ed53e68660b3fa1ebb", "model.onnx"), name: "model.onnx", sha256: "fea729ab6c5d338bc79ad3d986b073e6faa12c97255eafd78a1f7145326e534e", size: 238_434_293, unpack: Unpack::None }];

const OWLV2: &[FileSpec] = &[
    FileSpec { url: hf!("onnx-community/owlv2-base-patch16-ensemble-ONNX", "180d6ef7599bd69bb48db5c72bd49ad03ddfab80", "onnx/model_fp16.onnx"), name: "model_fp16.onnx", sha256: "694e2ae55306381ec0643edeb272ba6a4987820cb578ff38323210bc89fdb96d", size: 307_407_627, unpack: Unpack::None },
    FileSpec { url: hf!("onnx-community/owlv2-base-patch16-ensemble-ONNX", "180d6ef7599bd69bb48db5c72bd49ad03ddfab80", "tokenizer.json"), name: "tokenizer.json", sha256: "e277946093d72c7748281a6a344d6c79a5226c48954d6797dc36984aea23ac60", size: 3_642_208, unpack: Unpack::None },
];

const SIGLIP: &[FileSpec] = &[
    FileSpec { url: hf!("Xenova/siglip-base-patch16-224", "4649052661e53c7000355844105f8a1792088239", "onnx/vision_model_fp16.onnx"), name: "vision_model.onnx", sha256: "7287a8e9cbf4f2eedd66e8dc5cf584be03108f4e4a24331879ac6ef7ac7c878f", size: 186_142_673, unpack: Unpack::None },
    FileSpec { url: hf!("Xenova/siglip-base-patch16-224", "4649052661e53c7000355844105f8a1792088239", "onnx/text_model_fp16.onnx"), name: "text_model.onnx", sha256: "ebffbfd71ae11b3f9df5233095815953f44b02ca241c317722edd8c49a9de0f6", size: 220_867_426, unpack: Unpack::None },
    FileSpec { url: hf!("Xenova/siglip-base-patch16-224", "4649052661e53c7000355844105f8a1792088239", "tokenizer.json"), name: "tokenizer.json", sha256: "4a17c975210be5ab4c36b47d8dae4eefb866dbfb1e676e394aad85dc30a3ae08", size: 2_398_744, unpack: Unpack::None },
];

const OCR: &[FileSpec] = &[
    FileSpec { url: hf!("monkt/paddleocr-onnx", "7b02d0a30a07ba2b92ad1ff5a8941ae2c633de65", "detection/v3/det.onnx"), name: "det.onnx", sha256: "ee40e80071ba3a320d4efda75f3e22047a7d049e9bf7bcaaf9daea23fc21b935", size: 2_429_873, unpack: Unpack::None },
    FileSpec { url: hf!("monkt/paddleocr-onnx", "7b02d0a30a07ba2b92ad1ff5a8941ae2c633de65", "languages/latin/rec.onnx"), name: "rec.onnx", sha256: "614ffc2d6d3902d360fad7f1b0dd455ee45e877069d14c4e51a99dc4ef144409", size: 7_862_832, unpack: Unpack::None },
    FileSpec { url: hf!("monkt/paddleocr-onnx", "7b02d0a30a07ba2b92ad1ff5a8941ae2c633de65", "languages/latin/dict.txt"), name: "dict.txt", sha256: "3c0a8a79b612653c25f765271714f71281e4e955962c153e272b7b8c1d2b13ff", size: 1_634, unpack: Unpack::None },
];

const TRIPOSR: &[FileSpec] = &[
    FileSpec { url: hf!("fernandotonon/QtMeshEditor-triposr-onnx", "01b2f1add36601f964a89fd8a3159919ced4aa40", "triposr_encoder.onnx"), name: "encoder.onnx", sha256: "652b9bfe8851d6631b1e5a4532fa89fe9654086c0589c560b2bfc60cc5344bd7", size: 1_677_978_304, unpack: Unpack::None },
    FileSpec { url: hf!("fernandotonon/QtMeshEditor-triposr-onnx", "01b2f1add36601f964a89fd8a3159919ced4aa40", "triposr_decoder.onnx"), name: "decoder.onnx", sha256: "68ab06fb5ed4646c6dc15293ec83de62ba11d50294030795e860bea96cd81706", size: 180_692, unpack: Unpack::None },
];

const SPECS: &[ModelSpec] = &[
    ModelSpec { id: ModelId::WhisperTiny, dir: "whisper-tiny", name: "Whisper tiny (multilingual)", used_for: "Captions", licence: "MIT", source: "https://github.com/openai/whisper (ONNX export: huggingface.co/onnx-community/whisper-tiny_timestamped)", files: WHISPER_TINY },
    ModelSpec { id: ModelId::WhisperBase, dir: "whisper-base", name: "Whisper base (multilingual)", used_for: "Captions", licence: "MIT", source: "https://github.com/openai/whisper (ONNX export: huggingface.co/onnx-community/whisper-base_timestamped)", files: WHISPER_BASE },
    ModelSpec { id: ModelId::WhisperSmall, dir: "whisper-small", name: "Whisper small (multilingual)", used_for: "Captions", licence: "MIT", source: "https://github.com/openai/whisper (ONNX export: huggingface.co/onnx-community/whisper-small_timestamped)", files: WHISPER_SMALL },
    ModelSpec { id: ModelId::Modnet, dir: "modnet", name: "MODNet portrait matting", used_for: "Remove background", licence: "Apache-2.0", source: "https://github.com/ZHKKKe/MODNet (ONNX export: huggingface.co/Xenova/modnet)", files: MODNET },
    ModelSpec { id: ModelId::Sam21Tiny, dir: "sam2.1-tiny", name: "SAM 2.1 Hiera tiny", used_for: "Object cut-out", licence: "Apache-2.0", source: "https://github.com/facebookresearch/sam2 (ONNX export: huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX)", files: SAM21_TINY },
    ModelSpec { id: ModelId::DeepFilterNet3, dir: "deepfilternet3", name: "DeepFilterNet 3", used_for: "Voice cleanup", licence: "MIT OR Apache-2.0", source: "https://github.com/Rikorose/DeepFilterNet", files: DFN3 },
    ModelSpec { id: ModelId::Rife49, dir: "rife-4.9", name: "RIFE 4.9", used_for: "Smooth slow motion", licence: "MIT", source: "https://github.com/hzwer/Practical-RIFE (ONNX export: huggingface.co/yuvraj108c/rife-onnx)", files: RIFE49 },
    ModelSpec { id: ModelId::DepthAnythingV2Small, dir: "depth-anything-v2-small", name: "Depth Anything V2 small", used_for: "Depth effects", licence: "Apache-2.0", source: "https://github.com/DepthAnything/Depth-Anything-V2 (ONNX export: huggingface.co/onnx-community/depth-anything-v2-small)", files: DEPTH },
    ModelSpec { id: ModelId::FaceDetector, dir: "face-detector", name: "BlazeFace short range", used_for: "Face tracking", licence: "Apache-2.0", source: "https://ai.google.dev/edge/mediapipe/solutions/vision/face_detector", files: FACE },
    ModelSpec { id: ModelId::PoseLandmarker, dir: "pose-landmarker-lite", name: "BlazePose landmarker lite", used_for: "Body tracking", licence: "Apache-2.0", source: "https://ai.google.dev/edge/mediapipe/solutions/vision/pose_landmarker", files: POSE },
    ModelSpec { id: ModelId::RealEsrganGeneralX4v3, dir: "realesr-general-x4v3", name: "Real-ESRGAN general x4v3", used_for: "Upscale and enhance", licence: "BSD-3-Clause", source: "https://github.com/xinntao/Real-ESRGAN (ONNX export of the official weights: huggingface.co/vlad-grig/realesr-general-x4v3-onnx)", files: REALESR },
    ModelSpec { id: ModelId::UperNetConvNextTiny, dir: "upernet-convnext-tiny", name: "UperNet ConvNeXt-T (ADE20K scene parsing)", used_for: "Scene regions", licence: "MIT", source: "https://huggingface.co/openmmlab/upernet-convnext-tiny (ONNX export: huggingface.co/JElvis/upernet-convnext-tiny-ade-onnx)", files: UPERNET },
    ModelSpec { id: ModelId::OwlV2Base, dir: "owlv2-base-ensemble", name: "OWLv2 base (open-vocabulary detector)", used_for: "Object map", licence: "Apache-2.0", source: "https://github.com/google-research/scenic/tree/main/scenic/projects/owl_vit (checkpoint: huggingface.co/google/owlv2-base-patch16-ensemble; ONNX export: huggingface.co/onnx-community/owlv2-base-patch16-ensemble-ONNX)", files: OWLV2 },
    ModelSpec { id: ModelId::SiglipBase224, dir: "siglip-base-224", name: "SigLIP base (picture-text embeddings)", used_for: "Find a moment by description", licence: "Apache-2.0", source: "https://github.com/google-research/big_vision (checkpoint: huggingface.co/google/siglip-base-patch16-224; ONNX export: huggingface.co/Xenova/siglip-base-patch16-224)", files: SIGLIP },
    ModelSpec { id: ModelId::PaddleOcrLatin, dir: "paddleocr-latin", name: "PaddleOCR text finder and Latin reader", used_for: "Read text in the picture", licence: "Apache-2.0", source: "https://github.com/PaddlePaddle/PaddleOCR (PP-OCRv3 detector, PP-OCRv5 Latin recogniser; ONNX export: huggingface.co/monkt/paddleocr-onnx)", files: OCR },
    ModelSpec { id: ModelId::TripoSr, dir: "triposr", name: "TripoSR (picture to 3D model)", used_for: "Picture to 3D model", licence: "MIT", source: "https://github.com/VAST-AI-Research/TripoSR (weights: huggingface.co/stabilityai/TripoSR; ONNX export: huggingface.co/fernandotonon/QtMeshEditor-triposr-onnx)", files: TRIPOSR },
];

impl ModelId {
    pub const ALL: [ModelId; 16] = [
        ModelId::WhisperTiny,
        ModelId::WhisperBase,
        ModelId::WhisperSmall,
        ModelId::Modnet,
        ModelId::Sam21Tiny,
        ModelId::DeepFilterNet3,
        ModelId::Rife49,
        ModelId::DepthAnythingV2Small,
        ModelId::FaceDetector,
        ModelId::PoseLandmarker,
        ModelId::RealEsrganGeneralX4v3,
        ModelId::UperNetConvNextTiny,
        ModelId::OwlV2Base,
        ModelId::SiglipBase224,
        ModelId::PaddleOcrLatin,
        ModelId::TripoSr,
    ];

    pub fn spec(self) -> &'static ModelSpec {
        SPECS.iter().find(|s| s.id == self).expect("every ModelId has a spec")
    }

    /// Stable string id (the folder name), used in cache keys and settings.
    pub fn key(self) -> &'static str {
        self.spec().dir
    }

    pub fn from_key(key: &str) -> Option<ModelId> {
        Self::ALL.into_iter().find(|m| m.key() == key)
    }
}

/// All model specs.
pub fn all() -> &'static [ModelSpec] {
    SPECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_consistent() {
        let mut dirs = std::collections::HashSet::new();
        for id in ModelId::ALL {
            let s = id.spec();
            assert_eq!(s.id, id);
            assert!(dirs.insert(s.dir), "duplicate dir {}", s.dir);
            assert_eq!(ModelId::from_key(id.key()), Some(id));
            assert!(!s.files.is_empty());
            assert!(["MIT", "Apache-2.0", "MIT OR Apache-2.0", "BSD-3-Clause"].contains(&s.licence), "{} licence {}", s.name, s.licence);
            for f in s.files {
                assert!(f.url.starts_with("https://"), "{}", f.url);
                assert_eq!(f.sha256.len(), 64);
                assert!(f.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
                assert!(f.size > 0);
                for (name, sha, size) in f.installed_files() {
                    assert!(!name.contains('/') && !name.contains(".."), "{name}");
                    assert_eq!(sha.len(), 64);
                    assert!(size > 0);
                }
            }
        }
        assert_eq!(SPECS.len(), ModelId::ALL.len());
    }

    /// The object-knowledge models: permissive licences only, every file
    /// pinned to a revision, and no copyleft anywhere in the registry.
    #[test]
    fn the_object_models_are_licence_clean_and_pinned() {
        for id in [ModelId::OwlV2Base, ModelId::SiglipBase224, ModelId::PaddleOcrLatin] {
            let s = id.spec();
            assert_eq!(s.licence, "Apache-2.0", "{}", s.name);
            assert!(s.source.starts_with("https://github.com/"), "{}: name the upstream project", s.name);
            for f in s.files {
                let rev = f.url.split("/resolve/").nth(1).and_then(|r| r.split('/').next()).unwrap_or("");
                assert!(rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()), "{} is not pinned to a revision: {}", s.name, f.url);
            }
        }
        for s in all() {
            assert!(!s.licence.to_uppercase().contains("GPL"), "{} carries a copyleft licence: {}", s.name, s.licence);
        }
        // What a person is told to expect.
        assert_eq!(human_size(ModelId::OwlV2Base.spec().download_size()), "311 MB");
        assert_eq!(human_size(ModelId::SiglipBase224.spec().download_size()), "409 MB");
        assert_eq!(human_size(ModelId::PaddleOcrLatin.spec().download_size()), "10 MB");
        assert_eq!(ModelId::from_key("owlv2-base-ensemble"), Some(ModelId::OwlV2Base));
        assert_eq!(ModelId::from_key("siglip-base-224"), Some(ModelId::SiglipBase224));
        assert_eq!(ModelId::from_key("paddleocr-latin"), Some(ModelId::PaddleOcrLatin));
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human_size(229_746), "230 kB");
        assert_eq!(human_size(25_888_640), "26 MB");
        assert_eq!(human_size(1_500_000_000), "1.5 GB");
    }
}

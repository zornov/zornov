// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Camera log decoding: full-range normalised code value -> scene-linear
// reflectance. Mirrors `LogCurve::decode` in camera.rs step for step; ids
// match `LogCurve::shader_id`.

fn cam_ire(v: f32) -> f32 {
    return (v * 1023.0 - 64.0) / 876.0;
}

fn cam_pow10(x: f32) -> f32 {
    return exp2(x * 3.321928094887362);
}

fn camera_decode(id: u32, v: f32) -> f32 {
    switch id {
        // S-Log3
        case 0u: {
            if (v >= 171.2102946929 / 1023.0) { return cam_pow10((v * 1023.0 - 420.0) / 261.5) * 0.19 - 0.01; }
            return (v * 1023.0 - 95.0) * 0.01125 / (171.2102946929 - 95.0);
        }
        // LogC3 (EI 800)
        case 1u: {
            if (v > 5.367655 * 0.010591 + 0.092809) { return (cam_pow10((v - 0.385537) / 0.247190) - 0.052272) / 5.555556; }
            return (v - 0.092809) / 5.367655;
        }
        // LogC4
        case 2u: {
            let a = (262144.0 - 16.0) / 117.45;
            let b = (1023.0 - 95.0) / 1023.0;
            let c = 95.0 / 1023.0;
            let s = (7.0 * 0.6931471805599453 * exp2(7.0 - 14.0 * c / b)) / (a * b);
            let t = (exp2(14.0 * (-c / b) + 6.0) - 64.0) / a;
            if (v >= 0.0) { return (exp2(14.0 * (v - c) / b + 6.0) - 64.0) / a; }
            return v * s + t;
        }
        // V-Log
        case 3u: {
            if (v < 0.181) { return (v - 0.125) / 5.6; }
            return cam_pow10((v - 0.598206) / 0.241514) - 0.00873;
        }
        // Canon Log
        case 4u: {
            let x = cam_ire(v);
            if (x < 0.12512248) { return 0.9 * -(cam_pow10((0.12512248 - x) / 0.45310179) - 1.0) / 10.1596; }
            return 0.9 * (cam_pow10((x - 0.12512248) / 0.45310179) - 1.0) / 10.1596;
        }
        // Canon Log 2
        case 5u: {
            let x = cam_ire(v);
            if (x < 0.092864125) { return 0.9 * -(cam_pow10((0.092864125 - x) / 0.24136077) - 1.0) / 87.09937546; }
            return 0.9 * (cam_pow10((x - 0.092864125) / 0.24136077) - 1.0) / 87.09937546;
        }
        // Canon Log 3
        case 6u: {
            let x = cam_ire(v);
            if (x < 0.097465473) { return 0.9 * -(cam_pow10((0.12783901 - x) / 0.36726845) - 1.0) / 14.98325; }
            if (x <= 0.15277891) { return 0.9 * (x - 0.12512219) / 1.9754798; }
            return 0.9 * (cam_pow10((x - 0.12240537) / 0.36726845) - 1.0) / 14.98325;
        }
        // Apple Log
        case 7u: {
            let r0 = -0.05641088;
            let rt = 0.01;
            let sigma = 47.28711236;
            let pt = sigma * (rt - r0) * (rt - r0);
            if (v >= pt) { return exp2((v - 0.69336945) / 0.08550479) - 0.00964052; }
            if (v >= 0.0) { return sqrt(v / sigma) + r0; }
            return r0;
        }
        // RED Log3G10
        case 8u: {
            if (v < 0.0) { return v / 15.1927 - 0.01; }
            return (cam_pow10(v / 0.224282) - 1.0) / 155.975327 - 0.01;
        }
        // F-Log
        case 9u: {
            if (v < 0.100537775223865) { return (v - 0.092864) / 8.735631; }
            return cam_pow10((v - 0.790453) / 0.344676) / 0.555556 - 0.009468 / 0.555556;
        }
        // F-Log2
        case 10u: {
            if (v < 0.100686685370811) { return (v - 0.092864) / 8.799461; }
            return cam_pow10((v - 0.384316) / 0.245281) / 5.555556 - 0.064829 / 5.555556;
        }
        // DaVinci Intermediate
        case 11u: {
            if (v <= 0.02740668) { return v / 10.44426855; }
            return exp2(v / 0.07329248 - 7.0) - 0.0075;
        }
        // Blackmagic Film Gen 5
        case 12u: {
            if (v < 8.283605932402494 * 0.005 + 0.09246575342465753) { return (v - 0.09246575342465753) / 8.283605932402494; }
            return exp((v - 0.5300133392291939) / 0.08692876065491224) - 0.005494072432257808;
        }
        // ACEScct
        case 13u: {
            if (v <= 0.155251141552511) { return (v - 0.0729055341958355) / 10.5402377416545; }
            return exp2(v * 17.52 - 9.72);
        }
        // ACEScc
        case 14u: {
            if (v < (9.72 - 15.0) / 17.52) { return (exp2(v * 17.52 - 9.72) - exp2(-16.0)) * 2.0; }
            return min(exp2(v * 17.52 - 9.72), 65504.0);
        }
        // Cineon printing density
        default: {
            let black = cam_pow10((95.0 - 685.0) * 0.002 / 0.6);
            return (cam_pow10((v * 1023.0 - 685.0) * 0.002 / 0.6) - black) / (1.0 - black);
        }
    }
}

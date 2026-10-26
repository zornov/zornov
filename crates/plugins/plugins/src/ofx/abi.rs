// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The OpenFX C ABI: the structs and suite tables of the OFX Image Effect
//! API 1.4, written from the published specification
//! (<https://openeffects.org>). Names follow the C headers.

#![allow(non_camel_case_types, non_upper_case_globals, dead_code)]

use std::ffi::{c_char, c_double, c_int, c_uint, c_void};

pub type OfxStatus = c_int;
pub type OfxTime = c_double;
pub type Handle = *mut c_void;

pub const kOfxStatOK: OfxStatus = 0;
pub const kOfxStatFailed: OfxStatus = 1;
pub const kOfxStatErrFatal: OfxStatus = 2;
pub const kOfxStatErrUnknown: OfxStatus = 3;
pub const kOfxStatErrMissingHostFeature: OfxStatus = 4;
pub const kOfxStatErrUnsupported: OfxStatus = 5;
pub const kOfxStatErrExists: OfxStatus = 6;
pub const kOfxStatErrFormat: OfxStatus = 7;
pub const kOfxStatErrMemory: OfxStatus = 8;
pub const kOfxStatErrBadHandle: OfxStatus = 9;
pub const kOfxStatErrBadIndex: OfxStatus = 10;
pub const kOfxStatErrValue: OfxStatus = 11;
pub const kOfxStatReplyYes: OfxStatus = 12;
pub const kOfxStatReplyNo: OfxStatus = 13;
pub const kOfxStatReplyDefault: OfxStatus = 14;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OfxRectD {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OfxRangeD {
    pub min: f64,
    pub max: f64,
}

#[repr(C)]
pub struct OfxHost {
    pub host: Handle,
    pub fetch_suite: unsafe extern "C" fn(host: Handle, name: *const c_char, version: c_int) -> *const c_void,
}

#[repr(C)]
pub struct OfxPlugin {
    pub plugin_api: *const c_char,
    pub api_version: c_int,
    pub plugin_identifier: *const c_char,
    pub plugin_version_major: c_uint,
    pub plugin_version_minor: c_uint,
    pub set_host: Option<unsafe extern "C" fn(host: *mut OfxHost)>,
    pub main_entry: Option<unsafe extern "C" fn(action: *const c_char, handle: *const c_void, in_args: Handle, out_args: Handle) -> OfxStatus>,
}

#[repr(C)]
pub struct OfxPropertySuiteV1 {
    pub prop_set_pointer: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> OfxStatus,
    pub prop_set_string: unsafe extern "C" fn(Handle, *const c_char, c_int, *const c_char) -> OfxStatus,
    pub prop_set_double: unsafe extern "C" fn(Handle, *const c_char, c_int, c_double) -> OfxStatus,
    pub prop_set_int: unsafe extern "C" fn(Handle, *const c_char, c_int, c_int) -> OfxStatus,
    pub prop_set_pointer_n: unsafe extern "C" fn(Handle, *const c_char, c_int, *const *mut c_void) -> OfxStatus,
    pub prop_set_string_n: unsafe extern "C" fn(Handle, *const c_char, c_int, *const *const c_char) -> OfxStatus,
    pub prop_set_double_n: unsafe extern "C" fn(Handle, *const c_char, c_int, *const c_double) -> OfxStatus,
    pub prop_set_int_n: unsafe extern "C" fn(Handle, *const c_char, c_int, *const c_int) -> OfxStatus,
    pub prop_get_pointer: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_void) -> OfxStatus,
    pub prop_get_string: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_char) -> OfxStatus,
    pub prop_get_double: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_double) -> OfxStatus,
    pub prop_get_int: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_int) -> OfxStatus,
    pub prop_get_pointer_n: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_void) -> OfxStatus,
    pub prop_get_string_n: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_char) -> OfxStatus,
    pub prop_get_double_n: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_double) -> OfxStatus,
    pub prop_get_int_n: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_int) -> OfxStatus,
    pub prop_reset: unsafe extern "C" fn(Handle, *const c_char) -> OfxStatus,
    pub prop_get_dimension: unsafe extern "C" fn(Handle, *const c_char, *mut c_int) -> OfxStatus,
}

#[repr(C)]
pub struct OfxImageEffectSuiteV1 {
    pub get_property_set: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub get_param_set: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub clip_define: unsafe extern "C" fn(Handle, *const c_char, *mut Handle) -> OfxStatus,
    pub clip_get_handle: unsafe extern "C" fn(Handle, *const c_char, *mut Handle, *mut Handle) -> OfxStatus,
    pub clip_get_property_set: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub clip_get_image: unsafe extern "C" fn(Handle, OfxTime, *const OfxRectD, *mut Handle) -> OfxStatus,
    pub clip_release_image: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub clip_get_region_of_definition: unsafe extern "C" fn(Handle, OfxTime, *mut OfxRectD) -> OfxStatus,
    pub abort: unsafe extern "C" fn(Handle) -> c_int,
    pub image_memory_alloc: unsafe extern "C" fn(Handle, usize, *mut Handle) -> OfxStatus,
    pub image_memory_free: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub image_memory_lock: unsafe extern "C" fn(Handle, *mut *mut c_void) -> OfxStatus,
    pub image_memory_unlock: unsafe extern "C" fn(Handle) -> OfxStatus,
}

#[repr(C)]
pub struct OfxParameterSuiteV1 {
    pub param_define: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *mut Handle) -> OfxStatus,
    pub param_get_handle: unsafe extern "C" fn(Handle, *const c_char, *mut Handle, *mut Handle) -> OfxStatus,
    pub param_set_get_property_set: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub param_get_property_set: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub param_get_value: unsafe extern "C" fn(Handle, ...) -> OfxStatus,
    pub param_get_value_at_time: unsafe extern "C" fn(Handle, OfxTime, ...) -> OfxStatus,
    pub param_get_derivative: unsafe extern "C" fn(Handle, OfxTime, ...) -> OfxStatus,
    pub param_get_integral: unsafe extern "C" fn(Handle, OfxTime, OfxTime, ...) -> OfxStatus,
    pub param_set_value: unsafe extern "C" fn(Handle, ...) -> OfxStatus,
    pub param_set_value_at_time: unsafe extern "C" fn(Handle, OfxTime, ...) -> OfxStatus,
    pub param_get_num_keys: unsafe extern "C" fn(Handle, *mut c_uint) -> OfxStatus,
    pub param_get_key_time: unsafe extern "C" fn(Handle, c_uint, *mut OfxTime) -> OfxStatus,
    pub param_get_key_index: unsafe extern "C" fn(Handle, OfxTime, c_int, *mut c_int) -> OfxStatus,
    pub param_delete_key: unsafe extern "C" fn(Handle, OfxTime) -> OfxStatus,
    pub param_delete_all_keys: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub param_copy: unsafe extern "C" fn(Handle, Handle, OfxTime, *const OfxRangeD) -> OfxStatus,
    pub param_edit_begin: unsafe extern "C" fn(Handle, *const c_char) -> OfxStatus,
    pub param_edit_end: unsafe extern "C" fn(Handle) -> OfxStatus,
}

#[repr(C)]
pub struct OfxMemorySuiteV1 {
    pub memory_alloc: unsafe extern "C" fn(Handle, usize, *mut *mut c_void) -> OfxStatus,
    pub memory_free: unsafe extern "C" fn(*mut c_void) -> OfxStatus,
}

pub type OfxThreadFunctionV1 = unsafe extern "C" fn(c_uint, c_uint, *mut c_void);

#[repr(C)]
pub struct OfxMultiThreadSuiteV1 {
    pub multi_thread: unsafe extern "C" fn(OfxThreadFunctionV1, c_uint, *mut c_void) -> OfxStatus,
    pub multi_thread_num_cpus: unsafe extern "C" fn(*mut c_uint) -> OfxStatus,
    pub multi_thread_index: unsafe extern "C" fn(*mut c_uint) -> OfxStatus,
    pub multi_thread_is_spawned_thread: unsafe extern "C" fn() -> c_int,
    pub mutex_create: unsafe extern "C" fn(*mut Handle, c_int) -> OfxStatus,
    pub mutex_destroy: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub mutex_lock: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub mutex_unlock: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub mutex_try_lock: unsafe extern "C" fn(Handle) -> OfxStatus,
}

#[repr(C)]
pub struct OfxMessageSuiteV1 {
    pub message: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *const c_char, ...) -> OfxStatus,
}

#[repr(C)]
pub struct OfxMessageSuiteV2 {
    pub message: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *const c_char, ...) -> OfxStatus,
    pub set_persistent_message: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *const c_char, ...) -> OfxStatus,
    pub clear_persistent_message: unsafe extern "C" fn(Handle) -> OfxStatus,
}

// Strings used by the host. Plugins may use any property name; these are
// the ones Studio reads or sets.
pub const API: &str = "OfxImageEffectPluginAPI";

pub const ACTION_LOAD: &str = "OfxActionLoad";
pub const ACTION_UNLOAD: &str = "OfxActionUnload";
pub const ACTION_DESCRIBE: &str = "OfxActionDescribe";
pub const ACTION_DESCRIBE_IN_CONTEXT: &str = "OfxImageEffectActionDescribeInContext";
pub const ACTION_CREATE_INSTANCE: &str = "OfxActionCreateInstance";
pub const ACTION_DESTROY_INSTANCE: &str = "OfxActionDestroyInstance";
pub const ACTION_RENDER: &str = "OfxImageEffectActionRender";
pub const ACTION_BEGIN_SEQUENCE: &str = "OfxImageEffectActionBeginSequenceRender";
pub const ACTION_END_SEQUENCE: &str = "OfxImageEffectActionEndSequenceRender";
pub const ACTION_CLIP_PREFERENCES: &str = "OfxImageEffectActionGetClipPreferences";
pub const ACTION_IS_IDENTITY: &str = "OfxImageEffectActionIsIdentity";
pub const ACTION_INSTANCE_CHANGED: &str = "OfxActionInstanceChanged";
pub const ACTION_BEGIN_INSTANCE_CHANGED: &str = "OfxActionBeginInstanceChanged";
pub const ACTION_END_INSTANCE_CHANGED: &str = "OfxActionEndInstanceChanged";

pub const CONTEXT_FILTER: &str = "OfxImageEffectContextFilter";
pub const CONTEXT_GENERAL: &str = "OfxImageEffectContextGeneral";
pub const CONTEXT_GENERATOR: &str = "OfxImageEffectContextGenerator";
pub const CONTEXT_TRANSITION: &str = "OfxImageEffectContextTransition";

pub const DEPTH_FLOAT: &str = "OfxBitDepthFloat";
pub const DEPTH_BYTE: &str = "OfxBitDepthByte";
pub const DEPTH_SHORT: &str = "OfxBitDepthShort";
pub const RGBA: &str = "OfxImageComponentRGBA";

pub const P_LABEL: &str = "OfxPropLabel";
pub const P_NAME: &str = "OfxPropName";
pub const P_TIME: &str = "OfxPropTime";
pub const P_CONTEXT: &str = "OfxImageEffectPropContext";
pub const P_CONTEXTS: &str = "OfxImageEffectPropSupportedContexts";
pub const P_DEPTHS: &str = "OfxImageEffectPropSupportedPixelDepths";
pub const P_COMPONENTS: &str = "OfxImageEffectPropComponents";
pub const P_DEPTH: &str = "OfxImageEffectPropPixelDepth";
pub const P_GROUPING: &str = "OfxImageEffectPluginPropGrouping";
pub const P_DESCRIPTION: &str = "OfxPropPluginDescription";
pub const P_THREAD_SAFETY: &str = "OfxImageEffectPluginRenderThreadSafety";

pub const PARAM_DOUBLE: &str = "OfxParamTypeDouble";
pub const PARAM_INT: &str = "OfxParamTypeInteger";
pub const PARAM_BOOL: &str = "OfxParamTypeBoolean";
pub const PARAM_CHOICE: &str = "OfxParamTypeChoice";
pub const PARAM_RGBA: &str = "OfxParamTypeRGBA";
pub const PARAM_RGB: &str = "OfxParamTypeRGB";
pub const PARAM_DOUBLE2D: &str = "OfxParamTypeDouble2D";
pub const PARAM_INT2D: &str = "OfxParamTypeInteger2D";
pub const PARAM_DOUBLE3D: &str = "OfxParamTypeDouble3D";
pub const PARAM_INT3D: &str = "OfxParamTypeInteger3D";
pub const PARAM_STRING: &str = "OfxParamTypeString";
pub const PARAM_CUSTOM: &str = "OfxParamTypeCustom";
pub const PARAM_STRCHOICE: &str = "OfxParamTypeStrChoice";

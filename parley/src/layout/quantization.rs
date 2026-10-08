// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

#[cfg(feature = "libm")]
#[allow(unused_imports)]
use core_maths::CoreFloat;

/// Selects the coordinate space in which font metrics are quantized.
///
/// [`Device`](Self::Device) preserves Parley's existing behavior: metrics and
/// positions are rounded in layout (device) units. [`Logical`](Self::Logical)
/// rounds only font metrics in unscaled logical units, then scales them back
/// to layout units. It leaves authored geometry and accumulated positions
/// fractional.
///
/// Logical mode requires a finite, positive layout scale. Builders validate it
/// when building, including when quantization is disabled. Disabling
/// quantization leaves geometry unquantized in either mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuantizationMode {
    /// Quantize in layout (device) units.
    #[default]
    Device,
    /// Quantize font metrics in logical units before applying the layout scale.
    Logical,
}

/// Internal rounding policy for one layout.
///
/// A logical policy is constructed only after the builder has checked that its
/// scale is finite and positive.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Quantization {
    enabled: bool,
    mode: QuantizationMode,
    scale: f32,
}

impl Quantization {
    pub(crate) fn new(enabled: bool, mode: QuantizationMode, scale: f32) -> Self {
        Self {
            enabled,
            mode,
            scale,
        }
    }

    /// Logical font size for the opt-in hinted x-height fallback. Device and
    /// unquantized layouts retain their existing missing-metric behavior.
    pub(crate) fn x_height_measurement_size(self, font_size: f32) -> Option<f32> {
        (self.enabled && self.mode == QuantizationMode::Logical).then_some(font_size / self.scale)
    }

    /// Convert a measured logical font metric back to layout units.
    pub(crate) fn scale_font_metric(self, value: f32) -> f32 {
        value * self.scale
    }

    /// Round a font metric. Logical mode performs this in CSS/logical units.
    pub(crate) fn font_round(self, value: f32) -> f32 {
        if !self.enabled {
            return value;
        }
        match self.mode {
            QuantizationMode::Device => value.round(),
            QuantizationMode::Logical => (value / self.scale).round() * self.scale,
        }
    }

    /// Floor half-leading in the policy's font-metric coordinate space.
    pub(crate) fn half_leading_floor(self, value: f32) -> f32 {
        if !self.enabled {
            return value;
        }
        match self.mode {
            QuantizationMode::Device => value.floor(),
            QuantizationMode::Logical => (value / self.scale).floor() * self.scale,
        }
    }

    /// Round an accumulated or authored position only in device mode.
    pub(crate) fn position_round(self, value: f32) -> f32 {
        if self.enabled && self.mode == QuantizationMode::Device {
            value.round()
        } else {
            value
        }
    }

    /// [`Self::position_round`] for the f64 line cursor.
    pub(crate) fn position_round_f64(self, value: f64) -> f64 {
        if self.enabled && self.mode == QuantizationMode::Device {
            value.round()
        } else {
            value
        }
    }
}

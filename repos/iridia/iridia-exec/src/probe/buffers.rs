//! Read-only structured-buffer answers composed with a sampler's image answers.
use std::collections::BTreeMap;

use crate::{ImageLoad, ImageQuery, ImageSample, Load, Sample, Sampler};

/// The caller's stated view extent. The adapter validates the stride and addressable word count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferExtent {
    /// Bytes in an element, nonzero and divisible by four.
    pub stride: u32,
    pub elements: u32,
}

/// One word requested from a structured buffer, in the sampler's binding-slot vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferRead {
    pub slot: u32,
    pub element: u32,
    /// Byte offset within the element, aligned to a word.
    pub offset: u32,
}

/// A read-only sampler combining image answers (for example [`super::AnsweredReads`]) with finite structured views.
/// Successful structured reads retain both request and answer; failed reads stop with the request and reason.
/// Image reads are delegated unchanged. Stores and atomics are refused, even if `images` could answer them.
/// Buffer lengths come only from `extents`, never from an image sampler's absent-view policy.
///
/// An undeclared structured view is an error. An element beyond a declared extent reads zero, D3D's stated view
/// rule, without calling `answer`. An invalid offset within an element is always refused. The executor may use
/// [`Sampler::buffer_elements`] to resolve an out-of-range read before calling `structured`; `queries` records those
/// extent requests, while `reads` records only the structured calls actually made, not a complete instruction trace.
/// Lifted bounds guards can also read a valid element before masking its value away; the caller must answer every
/// requested in-range word, not only the element it expects the final output to use.
#[derive(Debug)]
pub struct AnsweredBuffers<S, F> {
    pub images: S,
    pub answer: F,
    extents: BTreeMap<u32, BufferExtent>,
    pub reads: Vec<(BufferRead, u32)>,
    pub queries: Vec<(u32, Option<u32>)>,
}

impl<S, F> AnsweredBuffers<S, F> {
    pub fn new(images: S, extents: BTreeMap<u32, BufferExtent>, answer: F) -> Result<Self, String> {
        for (&slot, extent) in &extents {
            if extent.stride == 0 || !extent.stride.is_multiple_of(4) {
                return Err(format!(
                    "t{slot}: structured stride {} is not a positive word multiple",
                    extent.stride
                ));
            }
            if u64::from(extent.stride / 4) * u64::from(extent.elements) > u64::from(u32::MAX) {
                return Err(format!(
                    "t{slot}: structured extent exceeds the executor's word address range"
                ));
            }
        }
        Ok(Self {
            images,
            answer,
            extents,
            reads: Vec::new(),
            queries: Vec::new(),
        })
    }
}

impl<S: Sampler, F: FnMut(BufferRead) -> Result<u32, String>> Sampler for AnsweredBuffers<S, F> {
    fn sample(&mut self, request: Sample) -> Result<[f32; 4], String> {
        self.images.sample(request)
    }

    fn load(&mut self, request: Load) -> Result<[u32; 4], String> {
        self.images.load(request)
    }

    fn sample_image(&mut self, request: ImageSample<u32>) -> Result<[f32; 4], String> {
        self.images.sample_image(request)
    }

    fn load_image(&mut self, request: ImageLoad<u32>) -> Result<[u32; 4], String> {
        self.images.load_image(request)
    }

    fn query_image(&mut self, request: ImageQuery<u32>) -> Result<[u32; 4], String> {
        self.images.query_image(request)
    }

    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> Result<u32, String> {
        let extent = self
            .extents
            .get(&slot)
            .ok_or_else(|| format!("t{slot}: no structured extent stated"))?;
        if !offset.is_multiple_of(4) || u64::from(offset) + 4 > u64::from(extent.stride) {
            return Err(format!(
                "t{slot}: structured byte offset {offset} outside word-aligned stride {}",
                extent.stride
            ));
        }
        let request = BufferRead {
            slot,
            element,
            offset,
        };
        let value = if element >= extent.elements {
            0
        } else {
            (self.answer)(request)
                .map_err(|why| format!("t{slot}: element {element} byte {offset}: {why}"))?
        };
        self.reads.push((request, value));
        Ok(value)
    }

    fn buffer_elements(&mut self, slot: u32) -> Option<u32> {
        let elements = self.extents.get(&slot).map(|e| e.elements);
        self.queries.push((slot, elements));
        elements
    }
}

#[cfg(test)]
#[path = "buffers_tests.rs"]
mod tests;

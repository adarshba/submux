//! Streaming primitives: SSE parse/emit, typed protocol event/chunk shapes,
//! tee, checkpoints, and the Anthropic↔OpenAI translator.

pub mod anthropic_events;
pub mod checkpoint;
pub mod openai_chunks;
pub mod sse_emitter;
pub mod sse_parser;
pub mod tee;
pub mod translate;

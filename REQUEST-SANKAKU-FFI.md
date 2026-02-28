# Request for Change: Export Sankaku Streaming FFI Surface

Date: 2026-02-28
Requester: Kagami reference client
Target: `sankaku-core`

## Current Verified State

Kagami now links against the precompiled shared libraries shipped in this repository:

- `dependencies/sankaku/libsankaku.dylib`
- `dependencies/nezumi/libnezumi.dylib`

Kagami does not import either project as a Rust crate, does not compile any code from `reference/`, and currently reaches Sankaku only through the dynamic library boundary.

That integration is working at the initialization level:

- Kagami links successfully against `libsankaku.dylib`.
- Kagami calls `init()` through an `extern "C"` declaration at process startup.
- The shipped `libsankaku.dylib` currently exposes only one global symbol relevant to Sankaku's own API surface: `_init`.

Using the `reference/` tree in read-only mode to verify the current Sankaku implementation, the streaming functionality already exists in Rust source form inside `sankaku-core`, including:

- `SankakuSender`
- `SankakuReceiver`
- `SankakuStream`
- `VideoFrame`
- `InboundVideoFrame`

However, those streaming types and methods are ordinary Rust API in `sankaku-core/src/session.rs`. They are not exported as C ABI entry points from the shipped dylib.

## Problem Statement

The current Sankaku dylib is sufficient for global initialization, but insufficient for media transport integration from Kagami.

At this point in time, Kagami can:

- load the Sankaku shared library;
- resolve and call `init()`.

Kagami cannot, through the dylib alone:

- construct a stream sender or receiver;
- attach Sankaku streaming state to a QUIC session through an FFI-safe handle;
- submit outbound `VideoFrame` payloads for transport;
- receive or poll `InboundVideoFrame` events;
- manage ownership of streaming buffers across the ABI boundary.

Because Kagami is required to consume Sankaku only as a dynamic library, these missing exports block all further streaming work.

## Requested Change

Add an explicit `extern "C"` FFI layer to `sankaku-core` for the streaming path.

### Required Scope

1. Add FFI-safe constructors and destructors for `SankakuStream`, or for equivalent sender and receiver handles.
   The constructor should accept an opaque QUIC connection handle, or an equivalent FFI-safe registration mechanism, instead of exposing Rust `quinn::Connection` or `Endpoint` types directly across the ABI.

2. Add FFI-safe functions to submit and recover video frame data.
   The API should cover the equivalent of `VideoFrame` input and `InboundVideoFrame` output using C-compatible structs, explicit pointer-plus-length parameters, and documented lifetime and free rules.

3. Add an FFI-safe polling or dequeue API for inbound video events.
   Kagami needs a way to retrieve `InboundVideoFrame` instances from Sankaku without depending on Rust channels, Tokio types, or async Rust signatures across the boundary.

4. Add explicit release functions for any heap-backed objects or buffers returned by the FFI.
   Ownership must remain unambiguous at the C ABI boundary.

## Justification

This request is necessary because the current binary export surface does not match the existing internal streaming implementation.

The read-only Sankaku source shows that the stream transport logic already exists in Rust, but the compiled dylib does not currently export that capability in a form Kagami can use. The result is a hard architectural mismatch:

- the functionality exists inside Sankaku;
- Kagami is only allowed to talk to Sankaku through `libsankaku.dylib`;
- the dylib presently exports only `init()`.

Kagami is the reference client and must preserve isolation by staying on the shared-library boundary. Importing `sankaku-core` directly, or compiling reference-source files into Kagami, would violate the no-contamination rule and defeat the intended separation between client and media engine.

An explicit C ABI layer is the correct fix because it:

- preserves Sankaku's existing internal Rust implementation;
- avoids leaking Rust-only types across the ABI;
- gives Kagami a stable binary contract instead of a source-level dependency;
- keeps memory ownership and lifecycle rules centralized inside Sankaku.

## Acceptance Criteria

1. `libsankaku.dylib` exports stream-related `extern "C"` symbols in addition to `init()`.
2. Kagami can create or attach a Sankaku streaming endpoint strictly through the dylib.
3. Kagami can submit outbound video frames without importing Sankaku Rust types.
4. Kagami can poll or dequeue inbound video frames through the dylib with documented ownership semantics.
5. No Rust-specific async, Tokio, or Quinn types cross the public FFI boundary directly.

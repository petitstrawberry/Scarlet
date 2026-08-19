//! Platform-neutral VirGL command code generation for SGFX.
//!
//! This crate owns shared VirGL dialect encoding only. Its helpers return
//! command bytes; it never opens devices, allocates operating-system resources,
//! selects a transport, synchronizes, submits commands, or presents images.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use sgfx_core::ir::{CullMode, FrontFace};

/// Encode the immutable state for the compatibility vertex-color pipeline.
///
/// Resource allocation and submission are deliberately absent: the caller
/// supplies transport-issued resource identifiers and submits the returned
/// bytes through its platform adapter.
///
/// # Arguments
///
/// * `image_resource_id` - VirGL resource identifier for the color target.
/// * `vertex_resource_id` - VirGL resource identifier for the vertex buffer.
/// * `vertex_stride` - Interleaved vertex stride in bytes.
/// * `cull_mode` - Triangle faces discarded by rasterization.
/// * `front_face` - Triangle winding considered front-facing.
///
/// # Returns
///
/// A complete VirGL command stream for immutable pipeline setup.
pub fn build_vertex_color_setup(
    image_resource_id: u32,
    vertex_resource_id: u32,
    vertex_stride: u32,
    cull_mode: CullMode,
    front_face: FrontFace,
) -> Vec<u8> {
    let mut commands = Vec::with_capacity(2_048);
    push_dword(
        &mut commands,
        command_header(CCMD_CREATE_OBJECT, OBJECT_SURFACE, 5),
    );
    push_dword(&mut commands, SURFACE_HANDLE);
    push_dword(&mut commands, image_resource_id);
    push_dword(&mut commands, FORMAT_B8G8R8A8_UNORM);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);

    push_dword(
        &mut commands,
        command_header(CCMD_CREATE_OBJECT, OBJECT_BLEND, 11),
    );
    push_dword(&mut commands, BLEND_HANDLE);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0xf << 27);
    for _ in 0..7 {
        push_dword(&mut commands, 0);
    }
    push_bind_object(&mut commands, OBJECT_BLEND, BLEND_HANDLE);

    push_dword(
        &mut commands,
        command_header(CCMD_CREATE_OBJECT, OBJECT_RASTERIZER, 9),
    );
    push_dword(&mut commands, RASTERIZER_HANDLE);
    push_dword(&mut commands, rasterizer_flags(cull_mode, front_face));
    push_float(&mut commands, 1.0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_float(&mut commands, 1.0);
    push_float(&mut commands, 0.0);
    push_float(&mut commands, 0.0);
    push_float(&mut commands, 0.0);
    push_bind_object(&mut commands, OBJECT_RASTERIZER, RASTERIZER_HANDLE);

    push_shader(
        &mut commands,
        VERTEX_SHADER_HANDLE,
        SHADER_VERTEX,
        VERTEX_SHADER,
    );
    push_bind_shader(&mut commands, VERTEX_SHADER_HANDLE, SHADER_VERTEX);
    push_shader(
        &mut commands,
        FRAGMENT_SHADER_HANDLE,
        SHADER_FRAGMENT,
        FRAGMENT_SHADER,
    );
    push_bind_shader(&mut commands, FRAGMENT_SHADER_HANDLE, SHADER_FRAGMENT);

    push_dword(
        &mut commands,
        command_header(CCMD_CREATE_OBJECT, OBJECT_VERTEX_ELEMENTS, 9),
    );
    push_dword(&mut commands, VERTEX_ELEMENTS_HANDLE);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, FORMAT_R32G32B32A32_FLOAT);
    push_dword(&mut commands, 16);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, FORMAT_R32G32B32_FLOAT);
    push_bind_object(
        &mut commands,
        OBJECT_VERTEX_ELEMENTS,
        VERTEX_ELEMENTS_HANDLE,
    );

    push_dword(
        &mut commands,
        command_header(CCMD_SET_FRAMEBUFFER_STATE, 0, 3),
    );
    push_dword(&mut commands, 1);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, SURFACE_HANDLE);

    push_dword(&mut commands, command_header(CCMD_SET_VERTEX_BUFFERS, 0, 3));
    push_dword(&mut commands, vertex_stride);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, vertex_resource_id);
    commands
}

const CCMD_CREATE_OBJECT: u32 = 1;
const CCMD_BIND_OBJECT: u32 = 2;
const CCMD_SET_FRAMEBUFFER_STATE: u32 = 5;
const CCMD_SET_VERTEX_BUFFERS: u32 = 6;
const CCMD_BIND_SHADER: u32 = 31;

const OBJECT_BLEND: u32 = 1;
const OBJECT_RASTERIZER: u32 = 2;
const OBJECT_SHADER: u32 = 4;
const OBJECT_VERTEX_ELEMENTS: u32 = 5;
const OBJECT_SURFACE: u32 = 8;

const FORMAT_B8G8R8A8_UNORM: u32 = 1;
const FORMAT_R32G32B32A32_FLOAT: u32 = 31;
const FORMAT_R32G32B32_FLOAT: u32 = 30;
const SHADER_VERTEX: u32 = 0;
const SHADER_FRAGMENT: u32 = 1;
const SHADER_TOKEN_COUNT_HINT: u32 = 300;

const SURFACE_HANDLE: u32 = 1;
const VERTEX_SHADER_HANDLE: u32 = 2;
const FRAGMENT_SHADER_HANDLE: u32 = 3;
const VERTEX_ELEMENTS_HANDLE: u32 = 4;
const BLEND_HANDLE: u32 = 5;
const RASTERIZER_HANDLE: u32 = 6;
const RASTERIZER_DEPTH_CLIP: u32 = 1 << 1;
const RASTERIZER_CULL_FACE_SHIFT: u32 = 8;
const RASTERIZER_FRONT_CCW: u32 = 1 << 15;

const VERTEX_SHADER: &str = "VERT\n\
DCL IN[0]\n\
DCL IN[1]\n\
DCL OUT[0], POSITION\n\
DCL OUT[1], COLOR\n\
  0: MOV OUT[0], IN[0]\n\
  1: MOV OUT[1], IN[1]\n\
  2: END\n";

const FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], COLOR, PERSPECTIVE\n\
DCL OUT[0], COLOR\n\
  0: MOV OUT[0], IN[0]\n\
   1: END\n";

const fn command_header(command: u32, object: u32, payload_dwords: u32) -> u32 {
    command | (object << 8) | (payload_dwords << 16)
}

fn push_dword(commands: &mut Vec<u8>, value: u32) {
    commands.extend_from_slice(&value.to_ne_bytes());
}

fn push_float(commands: &mut Vec<u8>, value: f32) {
    push_dword(commands, value.to_bits());
}

fn push_bind_object(commands: &mut Vec<u8>, object: u32, handle: u32) {
    push_dword(commands, command_header(CCMD_BIND_OBJECT, object, 1));
    push_dword(commands, handle);
}

fn push_bind_shader(commands: &mut Vec<u8>, handle: u32, shader_type: u32) {
    push_dword(commands, command_header(CCMD_BIND_SHADER, 0, 2));
    push_dword(commands, handle);
    push_dword(commands, shader_type);
}

fn push_shader(commands: &mut Vec<u8>, handle: u32, shader_type: u32, source: &str) {
    let source_bytes = source.as_bytes();
    let token_dwords = source_bytes.len().div_ceil(4) as u32;
    push_dword(
        commands,
        command_header(CCMD_CREATE_OBJECT, OBJECT_SHADER, 5 + token_dwords),
    );
    push_dword(commands, handle);
    push_dword(commands, shader_type);
    push_dword(commands, 0);
    push_dword(commands, source_bytes.len() as u32);
    push_dword(commands, SHADER_TOKEN_COUNT_HINT);
    commands.extend_from_slice(source_bytes);
    while !commands.len().is_multiple_of(4) {
        commands.push(0);
    }
}

const fn rasterizer_flags(cull_mode: CullMode, front_face: FrontFace) -> u32 {
    let cull_face = match cull_mode {
        CullMode::None => 0,
        CullMode::Front => 1,
        CullMode::Back => 2,
    } << RASTERIZER_CULL_FACE_SHIFT;
    let front_ccw = if matches!(front_face, FrontFace::CounterClockwise) {
        RASTERIZER_FRONT_CCW
    } else {
        0
    };
    RASTERIZER_DEPTH_CLIP | cull_face | front_ccw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(bytes: &[u8]) -> Vec<u32> {
        bytes
            .chunks_exact(4)
            .map(|bytes| u32::from_ne_bytes(bytes.try_into().expect("dword")))
            .collect()
    }

    #[test]
    fn setup_uses_transport_resource_ids_and_vertex_stride() {
        let commands =
            build_vertex_color_setup(41, 73, 28, CullMode::Back, FrontFace::CounterClockwise);
        let words = words(&commands);
        assert!(words.windows(6).any(|packet| {
            packet
                == [
                    command_header(CCMD_CREATE_OBJECT, OBJECT_SURFACE, 5),
                    SURFACE_HANDLE,
                    41,
                    FORMAT_B8G8R8A8_UNORM,
                    0,
                    0,
                ]
        }));
        assert!(words.windows(4).any(|packet| {
            packet == [command_header(CCMD_SET_VERTEX_BUFFERS, 0, 3), 28, 0, 73]
        }));
    }
}

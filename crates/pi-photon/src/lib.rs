//! pi-photon: runs the *exact* `@silvia-odwyer/photon-node` WASM build that the
//! original pi coding-agent uses for image processing.
//!
//! Byte-identical parity with the TypeScript implementation is achieved by
//! executing the same `photon_rs_bg.wasm` binary (built from
//! <https://github.com/silvia-odwyer/photon>, Apache-2.0) under
//! [wasmtime]. Wasm float semantics are deterministic across engines, so
//! decode/resize/encode output matches the Node.js (V8) execution bit for bit.
//!
//! A native recompile of the photon Rust sources is *not* byte-identical:
//! LLVM vectorizes the resize kernels differently on x86-64 vs wasm32, which
//! changes float rounding (verified empirically; see PARITY.md).
//!
//! Only the numeric wasm-bindgen ABI surface used by pi's image pipeline is
//! bound here (`PhotonImage` construction from bytes/raw pixels, dimension
//! queries, raw pixel access, PNG/JPEG encode, Lanczos3 resize, flips). The
//! browser-DOM wasm-bindgen imports are provided as trapping stubs — pi's
//! pipeline never reaches them.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// The WASM binary from `@silvia-odwyer/photon-node@0.3.4` (photon master,
/// built 2025-05). Kept byte-for-byte identical to the module the original
/// loads from its node_modules.
static WASM: &[u8] = include_bytes!("../wasm/photon_rs_bg.wasm");

/// Wasm-bindgen import module name for the placeholder (JS shim) imports.
const WBG: &str = "__wbindgen_placeholder__";

/// SamplingFilter::Lanczos3 (photon's numeric enum value).
const LANCZOS3: i32 = 5;

struct Runtime {
    store: wasmtime::Store<()>,
    memory: wasmtime::Memory,
    malloc: wasmtime::TypedFunc<(i32, i32), i32>,
    free: wasmtime::TypedFunc<(i32, i32, i32), ()>,
    new_from_byteslice: wasmtime::TypedFunc<(i32, i32), i32>,
    photonimage_new: wasmtime::TypedFunc<(i32, i32, i32, i32), i32>,
    photonimage_free: wasmtime::TypedFunc<(i32, i32), ()>,
    get_width: wasmtime::TypedFunc<i32, i32>,
    get_height: wasmtime::TypedFunc<i32, i32>,
    get_raw_pixels: wasmtime::TypedFunc<i32, (i32, i32)>,
    get_bytes: wasmtime::TypedFunc<i32, (i32, i32)>,
    get_bytes_jpeg: wasmtime::TypedFunc<(i32, i32), (i32, i32)>,
    resize: wasmtime::TypedFunc<(i32, i32, i32, i32), i32>,
    fliph: wasmtime::TypedFunc<i32, ()>,
    flipv: wasmtime::TypedFunc<i32, ()>,
}

fn runtime() -> MutexGuard<'static, Runtime> {
    static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(Runtime::new().expect("failed to initialize photon wasm runtime"))).lock().unwrap_or_else(|p| p.into_inner())
}

impl Runtime {
    fn new() -> anyhow::Result<Self> {
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, WASM)?;

        let mut store = wasmtime::Store::new(&engine, ());
        let mut linker = wasmtime::Linker::new(&engine);

        // ---- wasm-bindgen placeholder imports -------------------------------
        // All of these are only reachable from browser/panic code paths that
        // pi's image pipeline never takes. They trap if reached unexpectedly.
        fn stub(
            linker: &mut wasmtime::Linker<()>,
            store: &mut wasmtime::Store<()>,
            name: &str,
            params: &[wasmtime::ValType],
            results: &[wasmtime::ValType],
        ) -> anyhow::Result<()> {
            let ty = wasmtime::FuncType::new(store.engine(), params.iter().cloned(), results.iter().cloned());
            let label = name.to_string();
            linker.func_new(WBG, name, ty, move |_, _args, _results| {
                Err(anyhow::anyhow!("photon wasm called browser import {} (unreachable on this path)", label))
            })?;
            Ok(())
        }

        use wasmtime::ValType::{F64, I32};
        let e = [wasmtime::ValType::EXTERNREF];
        stub(&mut linker, &mut store, "__wbg_new_abda76e883ba8a5f", &[], &e)?;
        stub(&mut linker, &mut store, "__wbg_stack_658279fe44541cf6", &[I32, wasmtime::ValType::EXTERNREF], &[])?;
        stub(&mut linker, &mut store, "__wbg_error_f851667af71bcfc6", &[I32, I32], &[])?;
        stub(&mut linker, &mut store, "__wbg_instanceof_Window_c4b70662a0d2c5ec", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_document_e5c1786dea6542e4", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_body_e70ae6abd01ae584", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_createElement_5d4c76f218b78145", &[wasmtime::ValType::EXTERNREF, I32, I32], &e)?;
        stub(&mut linker, &mut store, "__wbg_width_4c6f0048d64cf86b", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_height_21f0d3fd8f753394", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_width_79e0847ed5883b03", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_height_e4e4e4779f8feac0", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_data_fda507064d127f5b", &[I32, wasmtime::ValType::EXTERNREF], &[])?;
        stub(&mut linker, &mut store, "__wbg_newwithu8clampedarrayandsh_1fddccb3a94a5e05", &[I32, I32, I32, I32], &e)?;
        stub(&mut linker, &mut store, "__wbg_instanceof_CanvasRenderingContext2d_3abbe7ec7af32cae", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_drawImage_fede06db74e39a60", &[wasmtime::ValType::EXTERNREF, wasmtime::ValType::EXTERNREF, F64, F64], &[])?;
        stub(
            &mut linker,
            &mut store,
            "__wbg_drawImage_f395c8e43c79a909",
            &[wasmtime::ValType::EXTERNREF, wasmtime::ValType::EXTERNREF, F64, F64, F64, F64, F64, F64, F64, F64],
            &[],
        )?;
        stub(&mut linker, &mut store, "__wbg_getImageData_5e1c242046e6b59e", &[wasmtime::ValType::EXTERNREF, F64, F64, F64, F64], &e)?;
        stub(&mut linker, &mut store, "__wbg_putImageData_a8b3e177ee06d521", &[wasmtime::ValType::EXTERNREF, wasmtime::ValType::EXTERNREF, F64, F64], &[])?;
        stub(&mut linker, &mut store, "__wbg_instanceof_HtmlCanvasElement_25d964a0dde6717e", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_width_dc225e55343b745e", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_setwidth_488780db69b08846", &[wasmtime::ValType::EXTERNREF, I32], &[])?;
        stub(&mut linker, &mut store, "__wbg_height_3a8bec2f3fe71b26", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_setheight_1761808c18403921", &[wasmtime::ValType::EXTERNREF, I32], &[])?;
        stub(&mut linker, &mut store, "__wbg_getContext_fc99dbd3a9a7e318", &[wasmtime::ValType::EXTERNREF, I32, I32], &[I32])?;
        stub(&mut linker, &mut store, "__wbg_settextContent_f82a86a8df347e1c", &[wasmtime::ValType::EXTERNREF, I32, I32], &[])?;
        stub(&mut linker, &mut store, "__wbg_appendChild_fa3b00dade9fc4cf", &[wasmtime::ValType::EXTERNREF, wasmtime::ValType::EXTERNREF], &e)?;
        stub(&mut linker, &mut store, "__wbg_newnoargs_e643855c6572a4a8", &[I32, I32], &e)?;
        stub(&mut linker, &mut store, "__wbg_call_f96b398515635514", &[wasmtime::ValType::EXTERNREF, wasmtime::ValType::EXTERNREF], &e)?;
        stub(&mut linker, &mut store, "__wbg_self_b9aad7f1c618bfaf", &[], &e)?;
        stub(&mut linker, &mut store, "__wbg_window_55e469842c98b086", &[], &e)?;
        stub(&mut linker, &mut store, "__wbg_globalThis_d0957e302752547e", &[], &e)?;
        stub(&mut linker, &mut store, "__wbg_global_ae2f87312b8987fb", &[], &e)?;
        stub(&mut linker, &mut store, "__wbindgen_is_undefined", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbg_buffer_fcbfb6d88b2732e9", &e, &e)?;
        stub(&mut linker, &mut store, "__wbg_new_bc5d9aad3f9ac80e", &e, &e)?;
        stub(&mut linker, &mut store, "__wbg_set_4b3aa8445ac1e91c", &[wasmtime::ValType::EXTERNREF, wasmtime::ValType::EXTERNREF, I32], &[])?;
        stub(&mut linker, &mut store, "__wbg_length_d9c4ded7e708c6a1", &e, &[I32])?;
        stub(&mut linker, &mut store, "__wbindgen_debug_string", &[I32, wasmtime::ValType::EXTERNREF], &[])?;
        // __wbindgen_throw(ptr, len): read the message and surface it.
        {
            let ty = wasmtime::FuncType::new(store.engine(), [I32, I32], []);
            linker.func_new(WBG, "__wbindgen_throw", ty, |mut caller, args, _| {
                let ptr = args[0].i32().unwrap_or_default() as u32 as usize;
                let len = args[1].i32().unwrap_or_default() as u32 as usize;
                let memory = caller.get_export("memory").and_then(|m| m.into_memory()).unwrap();
                let bytes = &memory.data(&caller)[ptr..ptr + len];
                let message = String::from_utf8_lossy(bytes).into_owned();
                Err(anyhow::anyhow!("photon wasm threw: {}", message))
            })?;
        }
        stub(&mut linker, &mut store, "__wbindgen_memory", &[], &e)?;
        // __wbindgen_init_externref_table(): no-op (externref table stays empty;
        // our path never stores into it).
        {
            let ty = wasmtime::FuncType::new(store.engine(), [], []);
            linker.func_new(WBG, "__wbindgen_init_externref_table", ty, |_, _, _| Ok(()))?;
        }

        let instance = linker.instantiate(&mut store, &module)?;
        instance
            .get_typed_func::<(), ()>(&mut store, "__wbindgen_start")?
            .call(&mut store, ())?;
        let memory = instance.get_memory(&mut store, "memory").unwrap();

        Ok(Runtime {
            malloc: instance.get_typed_func(&mut store, "__wbindgen_malloc")?,
            free: instance.get_typed_func(&mut store, "__wbindgen_free")?,
            new_from_byteslice: instance.get_typed_func(&mut store, "photonimage_new_from_byteslice")?,
            photonimage_new: instance.get_typed_func(&mut store, "photonimage_new")?,
            photonimage_free: instance.get_typed_func(&mut store, "__wbg_photonimage_free")?,
            get_width: instance.get_typed_func(&mut store, "photonimage_get_width")?,
            get_height: instance.get_typed_func(&mut store, "photonimage_get_height")?,
            get_raw_pixels: instance.get_typed_func(&mut store, "photonimage_get_raw_pixels")?,
            get_bytes: instance.get_typed_func(&mut store, "photonimage_get_bytes")?,
            get_bytes_jpeg: instance.get_typed_func(&mut store, "photonimage_get_bytes_jpeg")?,
            resize: instance.get_typed_func(&mut store, "resize")?,
            fliph: instance.get_typed_func(&mut store, "fliph")?,
            flipv: instance.get_typed_func(&mut store, "flipv")?,
            store,
            memory,
        })
    }

    /// Allocate `len` bytes in wasm memory (align 1, like passArray8ToWasm0)
    /// and copy `bytes` in. Returns the wasm pointer.
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<u32, String> {
        let len = i32::try_from(bytes.len()).map_err(|_| "photon: input too large".to_string())?;
        let ptr = self.malloc.call(&mut self.store, (len, 1)).map_err(trap_msg)?;
        self.memory
            .write(&mut self.store, ptr as usize, bytes)
            .map_err(|e| format!("photon: memory write failed: {}", e))?;
        Ok(ptr as u32)
    }

    /// Read `len` bytes at wasm `ptr` out of wasm memory.
    fn read_bytes(&self, ptr: u32, len: u32) -> Result<Vec<u8>, String> {
        self.memory
            .data(&self.store)
            .get(ptr as usize..(ptr as usize + len as usize))
            .map(|s| s.to_vec())
            .ok_or_else(|| "photon: out-of-range pointer from wasm".to_string())
    }

    /// Free a wasm-allocated buffer (like the JS glue's __wbindgen_free call).
    fn free_bytes(&mut self, ptr: u32, len: u32) {
        let _ = self.free.call(&mut self.store, (ptr as i32, len as i32, 1));
    }
}

fn trap_msg(err: anyhow::Error) -> String {
    format!("photon wasm error: {}", err)
}

/// A `PhotonImage` living in the shared wasm runtime, mirroring the JS class
/// exposed by `@silvia-odwyer/photon-node`.
pub struct PhotonImage {
    /// Raw wasm-bindgen object pointer (the JS `__wbg_ptr`).
    handle: u32,
}

impl PhotonImage {
    /// `PhotonImage.new_from_byteslice(vec)` — decode bytes (PNG/JPEG/GIF/
    /// WebP/BMP) into RGBA pixels.
    pub fn new_from_byteslice(bytes: &[u8]) -> Result<PhotonImage, String> {
        let mut rt = runtime();
        let ptr = rt.write_bytes(bytes)?;
        // wasm-bindgen transfers ownership of the input buffer to the wasm
        // side (the JS glue does not free it either); freeing here would
        // corrupt the wasm heap.
        let f = rt.new_from_byteslice.clone();
        let result = f.call(&mut rt.store, (ptr as i32, bytes.len() as i32));
        let handle = result.map_err(trap_msg)? as u32;
        if handle == 0 {
            return Err("photon: new_from_byteslice failed (unsupported image data)".to_string());
        }
        Ok(PhotonImage { handle })
    }

    /// `new PhotonImage(rawPixels, width, height)` — construct from RGBA pixels.
    pub fn from_raw_pixels(pixels: &[u8], width: u32, height: u32) -> Result<PhotonImage, String> {
        let mut rt = runtime();
        let ptr = rt.write_bytes(pixels)?;
        // Ownership of the input buffer transfers to the wasm side.
        let f = rt.photonimage_new.clone();
        let result = f.call(&mut rt.store, (ptr as i32, pixels.len() as i32, width as i32, height as i32));
        let handle = result.map_err(trap_msg)? as u32;
        if handle == 0 {
            return Err("photon: PhotonImage::new failed".to_string());
        }
        Ok(PhotonImage { handle })
    }

    pub fn width(&self) -> u32 {
        let mut rt = runtime();
        let f = rt.get_width.clone();
        f.call(&mut rt.store, self.handle as i32).unwrap_or(0) as u32
    }

    pub fn height(&self) -> u32 {
        let mut rt = runtime();
        let f = rt.get_height.clone();
        f.call(&mut rt.store, self.handle as i32).unwrap_or(0) as u32
    }

    /// `get_raw_pixels()` — RGBA pixel bytes.
    pub fn raw_pixels(&self) -> Result<Vec<u8>, String> {
        let mut rt = runtime();
        let f = rt.get_raw_pixels.clone();
        let (ptr, len) = f.call(&mut rt.store, self.handle as i32).map_err(trap_msg)?;
        let (ptr, len) = (ptr as u32, len as u32);
        let bytes = rt.read_bytes(ptr, len)?;
        rt.free_bytes(ptr, len);
        Ok(bytes)
    }

    /// `get_bytes()` — PNG-encoded bytes.
    pub fn get_bytes(&self) -> Result<Vec<u8>, String> {
        let mut rt = runtime();
        let f = rt.get_bytes.clone();
        let (ptr, len) = f.call(&mut rt.store, self.handle as i32).map_err(trap_msg)?;
        let (ptr, len) = (ptr as u32, len as u32);
        let bytes = rt.read_bytes(ptr, len)?;
        rt.free_bytes(ptr, len);
        Ok(bytes)
    }

    /// `get_bytes_jpeg(quality)` — JPEG-encoded bytes.
    pub fn get_bytes_jpeg(&self, quality: u8) -> Result<Vec<u8>, String> {
        let mut rt = runtime();
        let f = rt.get_bytes_jpeg.clone();
        let (ptr, len) = f.call(&mut rt.store, (self.handle as i32, quality as i32)).map_err(trap_msg)?;
        let (ptr, len) = (ptr as u32, len as u32);
        let bytes = rt.read_bytes(ptr, len)?;
        rt.free_bytes(ptr, len);
        Ok(bytes)
    }

    /// `photon.resize(img, width, height, SamplingFilter.Lanczos3)`.
    pub fn resize(&self, width: u32, height: u32) -> Result<PhotonImage, String> {
        let mut rt = runtime();
        let f = rt.resize.clone();
        let handle = f
            .call(&mut rt.store, (self.handle as i32, width as i32, height as i32, LANCZOS3))
            .map_err(trap_msg)? as u32;
        if handle == 0 {
            return Err("photon: resize failed".to_string());
        }
        Ok(PhotonImage { handle })
    }

    /// `photon.fliph(img)` — horizontal flip, in place.
    pub fn fliph(&self) -> Result<(), String> {
        let mut rt = runtime();
        let f = rt.fliph.clone();
        f.call(&mut rt.store, self.handle as i32).map_err(trap_msg)
    }

    /// `photon.flipv(img)` — vertical flip, in place.
    pub fn flipv(&self) -> Result<(), String> {
        let mut rt = runtime();
        let f = rt.flipv.clone();
        f.call(&mut rt.store, self.handle as i32).map_err(trap_msg)
    }
}

impl Drop for PhotonImage {
    fn drop(&mut self) {
        let mut rt = runtime();
        // Explicit free (flag 0), like the JS `free()` method.
        let f = rt.photonimage_free.clone();
        let _ = f.call(&mut rt.store, (self.handle as i32, 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny 1x1 PNG: single red pixel.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn decodes_png_dimensions_and_pixels() {
        let img = PhotonImage::new_from_byteslice(TINY_PNG).unwrap();
        assert_eq!((img.width(), img.height()), (1, 1));
        let pixels = img.raw_pixels().unwrap();
        assert_eq!(pixels.len(), 4);
        assert_eq!(&pixels[..3], &[0xff, 0x00, 0x00]);
    }

    #[test]
    fn encodes_png_roundtrip() {
        let img = PhotonImage::new_from_byteslice(TINY_PNG).unwrap();
        let bytes = img.get_bytes().unwrap();
        // Output is a valid PNG with the same signature.
        assert_eq!(&bytes[..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
        // And it decodes back to the same pixels.
        let again = PhotonImage::new_from_byteslice(&bytes).unwrap();
        assert_eq!(again.raw_pixels().unwrap(), img.raw_pixels().unwrap());
    }

    #[test]
    fn from_raw_pixels_and_flips() {
        // 2x1: [red, green]
        let pixels = [255, 0, 0, 255, 0, 255, 0, 255];
        let img = PhotonImage::from_raw_pixels(&pixels, 2, 1).unwrap();
        assert_eq!((img.width(), img.height()), (2, 1));
        img.fliph().unwrap();
        assert_eq!(img.raw_pixels().unwrap(), [0, 255, 0, 255, 255, 0, 0, 255]);
        img.flipv().unwrap(); // height 1: no change
        assert_eq!(img.raw_pixels().unwrap(), [0, 255, 0, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn resize_scales_down() {
        let pixels: Vec<u8> = (0..4 * 4 * 4).map(|i| if i % 4 == 3 { 255 } else { (i * 7) as u8 }).collect();
        let img = PhotonImage::from_raw_pixels(&pixels, 4, 4).unwrap();
        let resized = img.resize(2, 2).unwrap();
        assert_eq!((resized.width(), resized.height()), (2, 2));
        assert_eq!(resized.raw_pixels().unwrap().len(), 2 * 2 * 4);
    }

    #[test]
    fn invalid_bytes_error() {
        assert!(PhotonImage::new_from_byteslice(b"not an image").is_err());
    }
}

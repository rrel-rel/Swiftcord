//! Draws egui's output with Direct3D 11: a flip-model swap chain on the window, one shader pair, and egui's
//! textures as plain RGBA textures. D3D11 is what Windows' own UI stack (Direct2D, XAML) sits on, so it keeps
//! the driver's footprint far below a D3D12/Vulkan renderer's.

use std::collections::HashMap;

use egui::epaint::{ClippedPrimitive, ImageDelta, Primitive, TextureId, Vertex};
use egui::{TextureFilter, TextureOptions, TextureWrapMode};
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::{Interface, s};

const VERTEX_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/egui_vs.cso"));
const PIXEL_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/egui_ps.cso"));

struct Texture {
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
    options: TextureOptions,
}

/// A growable dynamic buffer, rewritten every frame.
struct DynamicBuffer {
    buffer: Option<ID3D11Buffer>,
    capacity: usize,
    bind: D3D11_BIND_FLAG,
}

impl DynamicBuffer {
    fn new(bind: D3D11_BIND_FLAG) -> Self {
        Self { buffer: None, capacity: 0, bind }
    }

    fn write(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, bytes: &[u8]) -> windows::core::Result<()> {
        if bytes.len() > self.capacity || self.buffer.is_none() {
            self.capacity = bytes.len().next_power_of_two().max(64 * 1024);
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: self.capacity as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: self.bind.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            self.buffer = None;
            unsafe { device.CreateBuffer(&desc, None, Some(&mut self.buffer))? };
        }
        let buffer = self.buffer.as_ref().unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.pData as *mut u8, bytes.len());
            context.Unmap(buffer, 0);
        }
        Ok(())
    }
}

pub struct Painter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain1,
    target: Option<ID3D11RenderTargetView>,
    size: [u32; 2],
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    input_layout: ID3D11InputLayout,
    blend: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    screen: ID3D11Buffer,
    vertices: DynamicBuffer,
    indices: DynamicBuffer,
    samplers: HashMap<(TextureFilter, TextureFilter, TextureWrapMode), ID3D11SamplerState>,
    textures: HashMap<TextureId, Texture>,
}

impl Painter {
    pub fn new(hwnd: HWND, size: [u32; 2]) -> windows::core::Result<Self> {
        let (device, context) = create_device()?;
        let size = [size[0].max(1), size[1].max(1)];

        let factory: IDXGIFactory2 = unsafe { device.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()? };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size[0],
            Height: size[1],
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let swap_chain = unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)? };
        // Fullscreen is the window's business (a borderless window), not DXGI's.
        unsafe { factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)? };

        let mut vertex_shader = None;
        let mut pixel_shader = None;
        let mut input_layout = None;
        let mut blend = None;
        let mut rasterizer = None;
        let mut screen = None;
        unsafe {
            device.CreateVertexShader(VERTEX_SHADER, None, Some(&mut vertex_shader))?;
            device.CreatePixelShader(PIXEL_SHADER, None, Some(&mut pixel_shader))?;
            let element = |name, format, offset| D3D11_INPUT_ELEMENT_DESC {
                SemanticName: name,
                SemanticIndex: 0,
                Format: format,
                InputSlot: 0,
                AlignedByteOffset: offset,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            };
            // egui's Vertex: pos (2 × f32), uv (2 × f32), color (4 × u8).
            let layout = [
                element(s!("POSITION"), DXGI_FORMAT_R32G32_FLOAT, 0),
                element(s!("TEXCOORD"), DXGI_FORMAT_R32G32_FLOAT, 8),
                element(s!("COLOR"), DXGI_FORMAT_R8G8B8A8_UNORM, 16),
            ];
            device.CreateInputLayout(&layout, VERTEX_SHADER, Some(&mut input_layout))?;

            // Premultiplied alpha.
            let mut blend_desc = D3D11_BLEND_DESC::default();
            blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_INV_DEST_ALPHA,
                DestBlendAlpha: D3D11_BLEND_ONE,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            device.CreateBlendState(&blend_desc, Some(&mut blend))?;

            let rasterizer_desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                ScissorEnable: true.into(),
                DepthClipEnable: false.into(),
                ..Default::default()
            };
            device.CreateRasterizerState(&rasterizer_desc, Some(&mut rasterizer))?;

            let screen_desc = D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            device.CreateBuffer(&screen_desc, None, Some(&mut screen))?;
        }

        let mut painter = Self {
            device,
            context,
            swap_chain,
            target: None,
            size,
            vertex_shader: vertex_shader.unwrap(),
            pixel_shader: pixel_shader.unwrap(),
            input_layout: input_layout.unwrap(),
            blend: blend.unwrap(),
            rasterizer: rasterizer.unwrap(),
            screen: screen.unwrap(),
            vertices: DynamicBuffer::new(D3D11_BIND_VERTEX_BUFFER),
            indices: DynamicBuffer::new(D3D11_BIND_INDEX_BUFFER),
            samplers: HashMap::new(),
            textures: HashMap::new(),
        };
        painter.create_target()?;
        Ok(painter)
    }

    /// The largest texture this device takes: 16384 from feature level 11_0, 8192 on older hardware.
    pub fn max_texture_side(&self) -> usize {
        if unsafe { self.device.GetFeatureLevel() }.0 >= windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0.0 { 16384 } else { 8192 }
    }

    fn create_target(&mut self) -> windows::core::Result<()> {
        let back: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0)? };
        let mut target = None;
        unsafe { self.device.CreateRenderTargetView(&back, None, Some(&mut target))? };
        self.target = target;
        Ok(())
    }

    /// Matches the swap chain to the window's new client size (in pixels).
    pub fn resize(&mut self, size: [u32; 2]) -> windows::core::Result<()> {
        let size = [size[0].max(1), size[1].max(1)];
        if size == self.size {
            return Ok(());
        }
        self.size = size;
        self.target = None;
        unsafe {
            self.context.OMSetRenderTargets(None, None);
            self.swap_chain.ResizeBuffers(0, size[0], size[1], DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        }
        self.create_target()
    }

    /// Applies egui's texture changes, draws the frame and presents it (waiting for vsync).
    pub fn paint(
        &mut self,
        pixels_per_point: f32,
        clear: [f32; 4],
        primitives: &[ClippedPrimitive],
        textures: &egui::TexturesDelta,
    ) -> windows::core::Result<()> {
        for (id, deltas) in &textures.set {
            for delta in deltas {
                if let Err(e) = self.set_texture(*id, delta) {
                    tracing::warn!("couldn't upload texture {id:?}: {e}");
                }
            }
        }
        self.draw(pixels_per_point, clear, primitives)?;
        for id in &textures.free {
            self.textures.remove(id);
        }
        unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)).ok() }
    }

    fn set_texture(&mut self, id: TextureId, delta: &ImageDelta) -> windows::core::Result<()> {
        let egui::ImageData::Color(image) = &delta.image;
        let [w, h] = image.size;
        if w == 0 || h == 0 {
            return Ok(());
        }
        let pixels: &[u8] = unsafe { std::slice::from_raw_parts(image.pixels.as_ptr().cast(), image.pixels.len() * 4) };
        let pitch = (w * 4) as u32;

        if let Some([x, y]) = delta.pos {
            // A patch of an existing texture (the font atlas grows this way).
            let Some(existing) = self.textures.get_mut(&id) else { return Ok(()) };
            existing.options = delta.options;
            let area = D3D11_BOX { left: x as u32, top: y as u32, front: 0, right: (x + w) as u32, bottom: (y + h) as u32, back: 1 };
            unsafe { self.context.UpdateSubresource(&existing.texture, 0, Some(&area), pixels.as_ptr().cast(), pitch, 0) };
            return Ok(());
        }

        let desc = D3D11_TEXTURE2D_DESC {
            Width: w as u32,
            Height: h as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let data = D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr().cast(), SysMemPitch: pitch, SysMemSlicePitch: 0 };
        let mut texture = None;
        let mut view = None;
        unsafe {
            self.device.CreateTexture2D(&desc, Some(&data), Some(&mut texture))?;
            self.device.CreateShaderResourceView(texture.as_ref().unwrap(), None, Some(&mut view))?;
        }
        self.textures.insert(id, Texture { texture: texture.unwrap(), view: view.unwrap(), options: delta.options });
        Ok(())
    }

    fn sampler(&mut self, options: TextureOptions) -> windows::core::Result<ID3D11SamplerState> {
        let key = (options.magnification, options.minification, options.wrap_mode);
        if let Some(s) = self.samplers.get(&key) {
            return Ok(s.clone());
        }
        let filter = match (options.minification, options.magnification) {
            (TextureFilter::Nearest, TextureFilter::Nearest) => D3D11_FILTER_MIN_MAG_MIP_POINT,
            (TextureFilter::Nearest, TextureFilter::Linear) => D3D11_FILTER_MIN_POINT_MAG_LINEAR_MIP_POINT,
            (TextureFilter::Linear, TextureFilter::Nearest) => D3D11_FILTER_MIN_LINEAR_MAG_MIP_POINT,
            (TextureFilter::Linear, TextureFilter::Linear) => D3D11_FILTER_MIN_MAG_LINEAR_MIP_POINT,
        };
        let address = match options.wrap_mode {
            TextureWrapMode::ClampToEdge => D3D11_TEXTURE_ADDRESS_CLAMP,
            TextureWrapMode::Repeat => D3D11_TEXTURE_ADDRESS_WRAP,
            TextureWrapMode::MirroredRepeat => D3D11_TEXTURE_ADDRESS_MIRROR,
        };
        let desc = D3D11_SAMPLER_DESC {
            Filter: filter,
            AddressU: address,
            AddressV: address,
            AddressW: address,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        unsafe { self.device.CreateSamplerState(&desc, Some(&mut sampler))? };
        let sampler = sampler.unwrap();
        self.samplers.insert(key, sampler.clone());
        Ok(sampler)
    }

    fn draw(&mut self, ppp: f32, clear: [f32; 4], primitives: &[ClippedPrimitive]) -> windows::core::Result<()> {
        let Some(target) = self.target.clone() else { return Ok(()) };
        let ctx = self.context.clone();
        let [w, h] = self.size;

        // Every mesh goes into one vertex and one index buffer; each draw picks its range.
        let meshes: Vec<_> = primitives
            .iter()
            .filter_map(|p| match &p.primitive {
                Primitive::Mesh(m) if !m.indices.is_empty() => Some((p.clip_rect, m)),
                _ => None,
            })
            .collect();
        let vertex_count: usize = meshes.iter().map(|(_, m)| m.vertices.len()).sum();
        let index_count: usize = meshes.iter().map(|(_, m)| m.indices.len()).sum();
        if vertex_count > 0 {
            let mut vertices: Vec<u8> = Vec::with_capacity(vertex_count * size_of::<Vertex>());
            let mut indices: Vec<u8> = Vec::with_capacity(index_count * 4);
            for (_, m) in &meshes {
                vertices.extend_from_slice(unsafe { std::slice::from_raw_parts(m.vertices.as_ptr().cast(), m.vertices.len() * size_of::<Vertex>()) });
                indices.extend_from_slice(unsafe { std::slice::from_raw_parts(m.indices.as_ptr().cast(), m.indices.len() * 4) });
            }
            self.vertices.write(&self.device, &ctx, &vertices)?;
            self.indices.write(&self.device, &ctx, &indices)?;
        }

        unsafe {
            let screen = [w as f32 / ppp, h as f32 / ppp, 0.0, 0.0];
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&self.screen, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            std::ptr::copy_nonoverlapping(screen.as_ptr(), mapped.pData as *mut f32, 4);
            ctx.Unmap(&self.screen, 0);

            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            ctx.ClearRenderTargetView(&target, &clear);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
            ctx.RSSetState(&self.rasterizer);
            ctx.OMSetBlendState(&self.blend, None, u32::MAX);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(&self.input_layout);
            let stride = size_of::<Vertex>() as u32;
            let offset = 0u32;
            ctx.IASetVertexBuffers(0, 1, Some(&self.vertices.buffer), Some(&stride), Some(&offset));
            ctx.IASetIndexBuffer(self.indices.buffer.as_ref(), DXGI_FORMAT_R32_UINT, 0);
            ctx.VSSetShader(&self.vertex_shader, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.screen.clone())]));
            ctx.PSSetShader(&self.pixel_shader, None);
        }

        let (mut first_vertex, mut first_index) = (0usize, 0usize);
        for (clip, mesh) in meshes {
            let (base_vertex, start_index) = (first_vertex, first_index);
            first_vertex += mesh.vertices.len();
            first_index += mesh.indices.len();

            // Clip rect, in points, to whole pixels inside the target.
            let left = ((clip.min.x * ppp).round() as i32).clamp(0, w as i32);
            let top = ((clip.min.y * ppp).round() as i32).clamp(0, h as i32);
            let right = ((clip.max.x * ppp).round() as i32).clamp(left, w as i32);
            let bottom = ((clip.max.y * ppp).round() as i32).clamp(top, h as i32);
            if right == left || bottom == top {
                continue;
            }
            let Some((view, options)) = self.textures.get(&mesh.texture_id).map(|t| (t.view.clone(), t.options)) else {
                continue;
            };
            let sampler = self.sampler(options)?;
            unsafe {
                ctx.RSSetScissorRects(Some(&[RECT { left, top, right, bottom }]));
                ctx.PSSetShaderResources(0, Some(&[Some(view)]));
                ctx.PSSetSamplers(0, Some(&[Some(sampler)]));
                ctx.DrawIndexed(mesh.indices.len() as u32, start_index as u32, base_vertex as i32);
            }
        }
        Ok(())
    }
}

/// A hardware device, or Windows' software rasterizer (WARP) if there's no usable GPU.
fn create_device() -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut last = None;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device = None;
        let mut context = None;
        let result = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        match result {
            Ok(()) => return Ok((device.unwrap(), context.unwrap())),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap())
}

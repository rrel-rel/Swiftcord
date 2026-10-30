// egui's meshes: positions in points, colors and textures premultiplied and in gamma space. Blending happens in
// gamma space too, as egui expects, so the swap chain is plain UNORM.

cbuffer Screen : register(b0) {
    float2 screen_size_points;
    float2 padding;
};

struct VertexIn {
    float2 pos : POSITION;
    float2 uv : TEXCOORD;
    float4 color : COLOR;
};

struct PixelIn {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD;
    float4 color : COLOR;
};

Texture2D tex : register(t0);
SamplerState tex_sampler : register(s0);

PixelIn vs_main(VertexIn v) {
    PixelIn o;
    o.pos = float4(2.0 * v.pos.x / screen_size_points.x - 1.0, 1.0 - 2.0 * v.pos.y / screen_size_points.y, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    return o;
}

float4 ps_main(PixelIn p) : SV_TARGET {
    return p.color * tex.Sample(tex_sampler, p.uv);
}

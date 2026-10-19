/* A thin layer over libvpx's VP8 encoder, so the Rust side never has to match the layout of its C structs. */
#include <stdlib.h>
#include <string.h>
#include <vpx/vpx_encoder.h>
#include <vpx/vp8cx.h>

typedef void (*packet_fn)(void *user, const unsigned char *data, size_t len, int key, long long pts);

struct rec_enc {
    vpx_codec_ctx_t ctx;
    unsigned frame_ms;
};

struct rec_enc *rec_enc_new(unsigned w, unsigned h, unsigned fps, unsigned kbps) {
    vpx_codec_iface_t *iface = vpx_codec_vp8_cx();
    vpx_codec_enc_cfg_t cfg;
    if (vpx_codec_enc_config_default(iface, &cfg, 0)) return NULL;
    cfg.g_w = w;
    cfg.g_h = h;
    cfg.g_timebase.num = 1;
    cfg.g_timebase.den = 1000;
    cfg.g_threads = 4;
    cfg.g_lag_in_frames = 0;
    cfg.g_error_resilient = 0;
    cfg.rc_end_usage = VPX_VBR;
    cfg.rc_target_bitrate = kbps;
    cfg.kf_mode = VPX_KF_AUTO;
    cfg.kf_max_dist = fps * 5;
    struct rec_enc *enc = calloc(1, sizeof *enc);
    if (!enc) return NULL;
    enc->frame_ms = 1000 / (fps ? fps : 30);
    if (vpx_codec_enc_init(&enc->ctx, iface, &cfg, 0)) {
        free(enc);
        return NULL;
    }
    vpx_codec_control(&enc->ctx, VP8E_SET_CPUUSED, 8);
    return enc;
}

static int drain(struct rec_enc *enc, packet_fn write, void *user) {
    vpx_codec_iter_t iter = NULL;
    const vpx_codec_cx_pkt_t *pkt;
    while ((pkt = vpx_codec_get_cx_data(&enc->ctx, &iter))) {
        if (pkt->kind != VPX_CODEC_CX_FRAME_PKT) continue;
        write(user, pkt->data.frame.buf, pkt->data.frame.sz, (pkt->data.frame.flags & VPX_FRAME_IS_KEY) != 0, pkt->data.frame.pts);
    }
    return 0;
}

int rec_enc_frame(struct rec_enc *enc, const unsigned char *y, int ys, const unsigned char *u, int us, const unsigned char *v, int vs,
                  unsigned w, unsigned h, long long pts, int force_key, packet_fn write, void *user) {
    vpx_image_t img;
    memset(&img, 0, sizeof img);
    img.fmt = VPX_IMG_FMT_I420;
    img.w = img.d_w = w;
    img.h = img.d_h = h;
    img.x_chroma_shift = 1;
    img.y_chroma_shift = 1;
    img.bit_depth = 8;
    img.bps = 12;
    img.planes[0] = (unsigned char *)y;
    img.planes[1] = (unsigned char *)u;
    img.planes[2] = (unsigned char *)v;
    img.stride[0] = ys;
    img.stride[1] = us;
    img.stride[2] = vs;
    if (vpx_codec_encode(&enc->ctx, &img, pts, enc->frame_ms, force_key ? VPX_EFLAG_FORCE_KF : 0, VPX_DL_REALTIME)) return -1;
    return drain(enc, write, user);
}

int rec_enc_flush(struct rec_enc *enc, packet_fn write, void *user) {
    if (vpx_codec_encode(&enc->ctx, NULL, -1, 1, 0, VPX_DL_REALTIME)) return -1;
    return drain(enc, write, user);
}

void rec_enc_free(struct rec_enc *enc) {
    if (!enc) return;
    vpx_codec_destroy(&enc->ctx);
    free(enc);
}

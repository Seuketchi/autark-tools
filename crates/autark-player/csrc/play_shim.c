/* A thin layer over GStreamer's playbin: a URI in, decoded video frames pulled out as packed BGRx, sound played by
 * the default audio sink. Everything is polled from one thread, so Rust never sees a GStreamer object. */
#include <stdio.h>
#include <string.h>
#include <gst/gst.h>
#include <gst/app/gstappsink.h>
#include <gst/video/video.h>

struct player {
    GstElement *pipe;
    GstElement *sink;
    GstBus *bus;
    /* The frame shown after a seek or the first one arrives as a preroll sample, which isn't queued like the rest. */
    int want_preroll;
};

struct player *player_new(const char *uri, char *err, size_t errlen) {
    static int initialised;
    if (!initialised) {
        gst_init(NULL, NULL);
        initialised = 1;
    }
    GError *error = NULL;
    GstElement *video = gst_parse_bin_from_description(
        "videoconvert ! video/x-raw,format=BGRx ! appsink name=vsink max-buffers=2 drop=true sync=true", TRUE, &error);
    if (!video) {
        snprintf(err, errlen, "%s", error ? error->message : "no video sink");
        g_clear_error(&error);
        return NULL;
    }
    GstElement *pipe = gst_element_factory_make("playbin", "playbin");
    if (!pipe) {
        snprintf(err, errlen, "GStreamer has no playbin");
        gst_object_unref(video);
        return NULL;
    }
    g_object_set(pipe, "uri", uri, "video-sink", video, NULL);
    struct player *p = g_new0(struct player, 1);
    p->pipe = pipe;
    p->sink = gst_bin_get_by_name(GST_BIN(video), "vsink");
    p->bus = gst_element_get_bus(pipe);
    p->want_preroll = 1;
    /* Paused first: the duration and the first picture are there before anything plays. */
    gst_element_set_state(pipe, GST_STATE_PAUSED);
    return p;
}

void player_play(struct player *p, int playing) {
    gst_element_set_state(p->pipe, playing ? GST_STATE_PLAYING : GST_STATE_PAUSED);
}

/* 0 nothing, 1 end of stream, 2 error (text in err), 3 ready: prerolled, so the duration is known. */
int player_poll(struct player *p, char *err, size_t errlen) {
    GstMessage *msg = gst_bus_pop_filtered(p->bus, GST_MESSAGE_EOS | GST_MESSAGE_ERROR | GST_MESSAGE_ASYNC_DONE);
    if (!msg) return 0;
    int result = 0;
    switch (GST_MESSAGE_TYPE(msg)) {
    case GST_MESSAGE_EOS:
        result = 1;
        break;
    case GST_MESSAGE_ERROR: {
        GError *error = NULL;
        gst_message_parse_error(msg, &error, NULL);
        snprintf(err, errlen, "%s", error ? error->message : "playback failed");
        g_clear_error(&error);
        result = 2;
        break;
    }
    case GST_MESSAGE_ASYNC_DONE:
        result = 3;
        break;
    default:
        break;
    }
    gst_message_unref(msg);
    return result;
}

long long player_position_ms(struct player *p) {
    gint64 value;
    return gst_element_query_position(p->pipe, GST_FORMAT_TIME, &value) ? value / GST_MSECOND : -1;
}

long long player_duration_ms(struct player *p) {
    gint64 value;
    return gst_element_query_duration(p->pipe, GST_FORMAT_TIME, &value) ? value / GST_MSECOND : -1;
}

void player_seek_ms(struct player *p, long long ms) {
    if (ms < 0) ms = 0;
    gst_element_seek_simple(p->pipe, GST_FORMAT_TIME, GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_KEY_UNIT, ms * GST_MSECOND);
    p->want_preroll = 1;
}

void player_volume(struct player *p, double volume, int mute) {
    g_object_set(p->pipe, "volume", volume, "mute", mute ? TRUE : FALSE, NULL);
}

/* The newest picture, copied packed (width * 4 bytes a row) into dst. 1 when there is one, 0 when not, and -1 when dst
 * is too small, with the size in w and h: the picture is dropped, and the next one will fit. */
int player_frame(struct player *p, unsigned char *dst, size_t cap, int *w, int *h) {
    GstSample *latest = NULL, *next;
    while ((next = gst_app_sink_try_pull_sample(GST_APP_SINK(p->sink), 0))) {
        if (latest) gst_sample_unref(latest);
        latest = next;
    }
    if (!latest && p->want_preroll) {
        latest = gst_app_sink_try_pull_preroll(GST_APP_SINK(p->sink), 0);
        if (latest) p->want_preroll = 0;
    }
    if (!latest) return 0;
    GstVideoInfo info;
    if (!gst_video_info_from_caps(&info, gst_sample_get_caps(latest))) {
        gst_sample_unref(latest);
        return 0;
    }
    int width = GST_VIDEO_INFO_WIDTH(&info), height = GST_VIDEO_INFO_HEIGHT(&info);
    *w = width;
    *h = height;
    if ((size_t)width * height * 4 > cap) {
        gst_sample_unref(latest);
        return -1;
    }
    GstVideoFrame frame;
    if (!gst_video_frame_map(&frame, &info, gst_sample_get_buffer(latest), GST_MAP_READ)) {
        gst_sample_unref(latest);
        return 0;
    }
    const unsigned char *src = GST_VIDEO_FRAME_PLANE_DATA(&frame, 0);
    int stride = GST_VIDEO_FRAME_PLANE_STRIDE(&frame, 0);
    for (int y = 0; y < height; y++) memcpy(dst + (size_t)y * width * 4, src + (size_t)y * stride, (size_t)width * 4);
    gst_video_frame_unmap(&frame);
    gst_sample_unref(latest);
    return 1;
}

void player_free(struct player *p) {
    if (!p) return;
    gst_element_set_state(p->pipe, GST_STATE_NULL);
    gst_object_unref(p->sink);
    gst_object_unref(p->bus);
    gst_object_unref(p->pipe);
    g_free(p);
}

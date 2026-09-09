#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/error.h>
#include <libavutil/opt.h>
#include <libavutil/rational.h>

#define FS_US_TIME_BASE (AVRational){1, 1000000}

typedef enum DecoderBackend {
    BACKEND_SOFTWARE = 0,
    BACKEND_FFMPEG_MEDIACODEC = 1,
} DecoderBackend;

typedef struct FrameRow {
    int64_t timestamp_ticks;
    int64_t duration_ticks;
    int64_t dts_ticks;
    uint8_t has_timestamp;
    uint8_t has_duration;
    uint8_t has_dts;
    uint8_t keyframe;
    uint8_t corrupt;
} FrameRow;

typedef struct RowBuffer {
    FrameRow *rows;
    size_t len;
    size_t cap;
} RowBuffer;

typedef struct DecodeResult {
    const char *decoder_name;
    int codec_id;
    AVRational time_base;
    uint64_t frames;
    double open_ms;
    double wall_ms;
    double cpu_ms;
    double ttff_ms;
    double seek_settle_ms;
    int64_t first_timestamp_ticks;
    int64_t last_timestamp_ticks;
    int has_first_timestamp;
    int has_last_timestamp;
    RowBuffer rows;
} DecodeResult;

static double elapsed_ms(struct timespec start, struct timespec end) {
    const int64_t seconds = (int64_t)end.tv_sec - (int64_t)start.tv_sec;
    const int64_t nanos = (int64_t)end.tv_nsec - (int64_t)start.tv_nsec;
    return (double)seconds * 1000.0 + (double)nanos / 1000000.0;
}

static void print_av_error(const char *context, int code) {
    char message[AV_ERROR_MAX_STRING_SIZE];
    if (av_strerror(code, message, sizeof(message)) < 0) {
        snprintf(message, sizeof(message), "unknown FFmpeg error");
    }
    fprintf(stderr, "%s: %s (%d)\n", context, message, code);
}

static const char *decoder_name_for(enum AVCodecID codec_id, DecoderBackend backend) {
    switch (codec_id) {
        case AV_CODEC_ID_H264:
            return backend == BACKEND_FFMPEG_MEDIACODEC ? "h264_mediacodec" : "h264";
        case AV_CODEC_ID_HEVC:
            return backend == BACKEND_FFMPEG_MEDIACODEC ? "hevc_mediacodec" : "hevc";
        case AV_CODEC_ID_VP9:
            return backend == BACKEND_FFMPEG_MEDIACODEC ? "vp9_mediacodec" : "vp9";
        case AV_CODEC_ID_AV1:
            return backend == BACKEND_FFMPEG_MEDIACODEC ? "av1_mediacodec" : "av1";
        default:
            return NULL;
    }
}

static int append_row(RowBuffer *buffer, const FrameRow *row) {
    if (buffer->len == buffer->cap) {
        size_t next = buffer->cap == 0 ? 4096 : buffer->cap * 2;
        if (next < buffer->cap || next > SIZE_MAX / sizeof(FrameRow)) {
            return AVERROR(ENOMEM);
        }
        FrameRow *resized = (FrameRow *)realloc(buffer->rows, next * sizeof(FrameRow));
        if (resized == NULL) {
            return AVERROR(ENOMEM);
        }
        buffer->rows = resized;
        buffer->cap = next;
    }
    buffer->rows[buffer->len++] = *row;
    return 0;
}

static FrameRow frame_row(const AVFrame *frame) {
    FrameRow row;
    int64_t timestamp = frame->best_effort_timestamp;
    memset(&row, 0, sizeof(row));
    if (timestamp == AV_NOPTS_VALUE) {
        timestamp = frame->pts;
    }
    if (timestamp != AV_NOPTS_VALUE) {
        row.has_timestamp = 1;
        row.timestamp_ticks = timestamp;
    }
    if (frame->duration > 0) {
        row.has_duration = 1;
        row.duration_ticks = frame->duration;
    }
    if (frame->pkt_dts != AV_NOPTS_VALUE) {
        row.has_dts = 1;
        row.dts_ticks = frame->pkt_dts;
    }
    row.keyframe = (frame->flags & AV_FRAME_FLAG_KEY) != 0;
    row.corrupt = ((frame->flags & AV_FRAME_FLAG_CORRUPT) != 0) || frame->decode_error_flags != 0;
    return row;
}

static int first_video_stream(AVFormatContext *format) {
    unsigned int i;
    for (i = 0; i < format->nb_streams; ++i) {
        AVStream *stream = format->streams[i];
        if (stream != NULL && stream->codecpar != NULL && stream->codecpar->codec_type == AVMEDIA_TYPE_VIDEO) {
            return stream->index;
        }
    }
    return -1;
}

static AVStream *stream_by_index(AVFormatContext *format, int stream_index) {
    unsigned int i;
    for (i = 0; i < format->nb_streams; ++i) {
        if (format->streams[i] != NULL && format->streams[i]->index == stream_index) {
            return format->streams[i];
        }
    }
    return NULL;
}

static int receive_available(
    AVCodecContext *codec,
    AVFrame *frame,
    DecodeResult *out,
    uint64_t max_frames,
    int64_t seek_target_ticks,
    struct timespec wall_start,
    int *reached_limit,
    int *seek_settled
) {
    for (;;) {
        int result = avcodec_receive_frame(codec, frame);
        if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) {
            return result;
        }
        if (result < 0) {
            return result;
        }

        FrameRow row = frame_row(frame);
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (out->frames == 0) {
            out->ttff_ms = elapsed_ms(wall_start, now);
        }
        if (row.has_timestamp) {
            if (!out->has_first_timestamp) {
                out->first_timestamp_ticks = row.timestamp_ticks;
                out->has_first_timestamp = 1;
            }
            out->last_timestamp_ticks = row.timestamp_ticks;
            out->has_last_timestamp = 1;
            if (!*seek_settled && seek_target_ticks != AV_NOPTS_VALUE && row.timestamp_ticks >= seek_target_ticks) {
                out->seek_settle_ms = elapsed_ms(wall_start, now);
                *seek_settled = 1;
            }
        }
        result = append_row(&out->rows, &row);
        av_frame_unref(frame);
        if (result < 0) {
            return result;
        }
        out->frames += 1;
        if (max_frames > 0 && out->frames >= max_frames) {
            *reached_limit = 1;
            return 0;
        }
    }
}

static int decode_once(
    const char *path,
    DecoderBackend backend,
    uint64_t max_frames,
    int64_t seek_target_us,
    DecodeResult *out
) {
    AVFormatContext *format = NULL;
    AVCodecContext *codec = NULL;
    AVPacket *packet = NULL;
    AVFrame *frame = NULL;
    AVStream *stream = NULL;
    const AVCodec *decoder = NULL;
    struct timespec open_start, open_end, wall_start, wall_end, cpu_start, cpu_end;
    int stream_index;
    int result = 0;
    int reached_limit = 0;
    int seek_settled = 0;
    int64_t seek_target_ticks = AV_NOPTS_VALUE;

    memset(out, 0, sizeof(*out));
    out->ttff_ms = -1.0;
    out->seek_settle_ms = -1.0;
    clock_gettime(CLOCK_MONOTONIC, &open_start);

    result = avformat_open_input(&format, path, NULL, NULL);
    if (result < 0) {
        print_av_error("avformat_open_input", result);
        goto done;
    }
    result = avformat_find_stream_info(format, NULL);
    if (result < 0) {
        print_av_error("avformat_find_stream_info", result);
        goto done;
    }
    stream_index = first_video_stream(format);
    if (stream_index < 0) {
        fprintf(stderr, "source contains no video stream\n");
        result = AVERROR_STREAM_NOT_FOUND;
        goto done;
    }
    stream = stream_by_index(format, stream_index);
    if (stream == NULL || stream->codecpar == NULL) {
        result = AVERROR_STREAM_NOT_FOUND;
        goto done;
    }

    out->decoder_name = decoder_name_for(stream->codecpar->codec_id, backend);
    out->codec_id = stream->codecpar->codec_id;
    out->time_base = stream->time_base;
    if (out->decoder_name == NULL) {
        fprintf(stderr, "unsupported benchmark codec id %d\n", stream->codecpar->codec_id);
        result = AVERROR_DECODER_NOT_FOUND;
        goto done;
    }
    decoder = avcodec_find_decoder_by_name(out->decoder_name);
    if (decoder == NULL) {
        fprintf(stderr, "decoder %s is not present in this FFmpeg build\n", out->decoder_name);
        result = AVERROR_DECODER_NOT_FOUND;
        goto done;
    }
    codec = avcodec_alloc_context3(decoder);
    if (codec == NULL) {
        result = AVERROR(ENOMEM);
        goto done;
    }
    result = avcodec_parameters_to_context(codec, stream->codecpar);
    if (result < 0) {
        print_av_error("avcodec_parameters_to_context", result);
        goto done;
    }
    codec->pkt_timebase = stream->time_base;
    if (backend == BACKEND_FFMPEG_MEDIACODEC) {
        result = av_opt_set_int(codec->priv_data, "ndk_codec", 1, 0);
        if (result < 0) {
            print_av_error("forcing FFmpeg MediaCodec NDK backend", result);
            goto done;
        }
    }
    result = avcodec_open2(codec, decoder, NULL);
    if (result < 0) {
        print_av_error("avcodec_open2", result);
        goto done;
    }

    packet = av_packet_alloc();
    frame = av_frame_alloc();
    if (packet == NULL || frame == NULL) {
        result = AVERROR(ENOMEM);
        goto done;
    }
    clock_gettime(CLOCK_MONOTONIC, &open_end);
    out->open_ms = elapsed_ms(open_start, open_end);

    clock_gettime(CLOCK_MONOTONIC, &wall_start);
    clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &cpu_start);
    if (seek_target_us >= 0) {
        seek_target_ticks = av_rescale_q(seek_target_us, FS_US_TIME_BASE, stream->time_base);
        result = avformat_seek_file(
            format,
            stream_index,
            INT64_MIN,
            seek_target_ticks,
            seek_target_ticks,
            AVSEEK_FLAG_BACKWARD
        );
        if (result < 0) {
            print_av_error("avformat_seek_file", result);
            goto timed_done;
        }
        avcodec_flush_buffers(codec);
    }

    while (!reached_limit && (result = av_read_frame(format, packet)) >= 0) {
        if (packet->stream_index != stream_index) {
            av_packet_unref(packet);
            continue;
        }
        result = avcodec_send_packet(codec, packet);
        av_packet_unref(packet);
        if (result < 0 && result != AVERROR(EAGAIN)) {
            print_av_error("avcodec_send_packet", result);
            goto timed_done;
        }
        result = receive_available(
            codec,
            frame,
            out,
            max_frames,
            seek_target_ticks,
            wall_start,
            &reached_limit,
            &seek_settled
        );
        if (result < 0 && result != AVERROR(EAGAIN) && result != AVERROR_EOF) {
            print_av_error("avcodec_receive_frame", result);
            goto timed_done;
        }
    }
    if (!reached_limit && result != AVERROR_EOF) {
        print_av_error("av_read_frame", result);
        goto timed_done;
    }
    if (!reached_limit) {
        result = avcodec_send_packet(codec, NULL);
        if (result < 0 && result != AVERROR_EOF) {
            print_av_error("decoder flush", result);
            goto timed_done;
        }
        result = receive_available(
            codec,
            frame,
            out,
            max_frames,
            seek_target_ticks,
            wall_start,
            &reached_limit,
            &seek_settled
        );
        if (result < 0 && result != AVERROR_EOF && result != AVERROR(EAGAIN)) {
            print_av_error("decoder drain", result);
            goto timed_done;
        }
    }
    result = 0;

timed_done:
    clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &cpu_end);
    clock_gettime(CLOCK_MONOTONIC, &wall_end);
    out->wall_ms = elapsed_ms(wall_start, wall_end);
    out->cpu_ms = elapsed_ms(cpu_start, cpu_end);

done:
    av_frame_free(&frame);
    av_packet_free(&packet);
    avcodec_free_context(&codec);
    avformat_close_input(&format);
    return result;
}

static void free_result(DecodeResult *result) {
    free(result->rows.rows);
    result->rows.rows = NULL;
    result->rows.len = 0;
    result->rows.cap = 0;
}

static void print_result_json(const char *backend, const DecodeResult *result) {
    double fps = result->wall_ms > 0.0 ? (double)result->frames * 1000.0 / result->wall_ms : 0.0;
    printf(
        "{\"backend\":\"%s\",\"decoder\":\"%s\",\"codec_id\":%d,"
        "\"time_base_num\":%d,\"time_base_den\":%d,\"frames\":%" PRIu64 ","
        "\"open_ms\":%.3f,\"wall_ms\":%.3f,\"process_cpu_ms\":%.3f,"
        "\"ttff_ms\":%.3f,\"seek_settle_ms\":%.3f,\"fps\":%.3f}\n",
        backend,
        result->decoder_name != NULL ? result->decoder_name : "unknown",
        result->codec_id,
        result->time_base.num,
        result->time_base.den,
        result->frames,
        result->open_ms,
        result->wall_ms,
        result->cpu_ms,
        result->ttff_ms,
        result->seek_settle_ms,
        fps
    );
}

static int compare_rows(const DecodeResult *software, const DecodeResult *hardware) {
    size_t count = software->rows.len < hardware->rows.len ? software->rows.len : hardware->rows.len;
    size_t i;
    for (i = 0; i < count; ++i) {
        const FrameRow *a = &software->rows.rows[i];
        const FrameRow *b = &hardware->rows.rows[i];
        if (memcmp(a, b, sizeof(FrameRow)) != 0) {
            printf(
                "{\"equivalent\":false,\"reason\":\"frame_metadata_mismatch\",\"first_mismatch_frame\":%zu,"
                "\"software_pts\":%" PRId64 ",\"hardware_pts\":%" PRId64 ","
                "\"software_dts\":%" PRId64 ",\"hardware_dts\":%" PRId64 "}\n",
                i,
                a->has_timestamp ? a->timestamp_ticks : INT64_MIN,
                b->has_timestamp ? b->timestamp_ticks : INT64_MIN,
                a->has_dts ? a->dts_ticks : INT64_MIN,
                b->has_dts ? b->dts_ticks : INT64_MIN
            );
            return 2;
        }
    }
    if (software->rows.len != hardware->rows.len) {
        printf(
            "{\"equivalent\":false,\"reason\":\"frame_count_mismatch\",\"software_frames\":%zu,\"hardware_frames\":%zu}\n",
            software->rows.len,
            hardware->rows.len
        );
        return 2;
    }
    printf("{\"equivalent\":true,\"compared_frames\":%zu}\n", count);
    return 0;
}

static DecoderBackend parse_backend(const char *value, const char **label) {
    if (strcmp(value, "software") == 0) {
        *label = "software";
        return BACKEND_SOFTWARE;
    }
    if (strcmp(value, "ffmpeg-mediacodec") == 0) {
        *label = "ffmpeg-mediacodec";
        return BACKEND_FFMPEG_MEDIACODEC;
    }
    fprintf(stderr, "unknown backend: %s\n", value);
    exit(64);
}

static uint64_t parse_max_frames(const char *value) {
    char *end = NULL;
    errno = 0;
    unsigned long long parsed = strtoull(value, &end, 10);
    if (errno != 0 || end == value || *end != '\0') {
        fprintf(stderr, "invalid max frame count: %s\n", value);
        exit(64);
    }
    return (uint64_t)parsed;
}

int main(int argc, char **argv) {
    DecodeResult first;
    DecodeResult second;
    int result;

    if (argc < 3) {
        fprintf(
            stderr,
            "usage:\n"
            "  %s compare <input> [max_frames]\n"
            "  %s decode <software|ffmpeg-mediacodec> <input> [max_frames]\n"
            "  %s seek <software|ffmpeg-mediacodec> <input> <target_us> [max_frames]\n",
            argv[0], argv[0], argv[0]
        );
        return 64;
    }

    if (strcmp(argv[1], "compare") == 0) {
        uint64_t max_frames = argc >= 4 ? parse_max_frames(argv[3]) : 0;
        result = decode_once(argv[2], BACKEND_SOFTWARE, max_frames, -1, &first);
        if (result < 0) {
            free_result(&first);
            return 1;
        }
        result = decode_once(argv[2], BACKEND_FFMPEG_MEDIACODEC, max_frames, -1, &second);
        if (result < 0) {
            free_result(&first);
            free_result(&second);
            return 1;
        }
        print_result_json("software", &first);
        print_result_json("ffmpeg-mediacodec", &second);
        result = compare_rows(&first, &second);
        free_result(&first);
        free_result(&second);
        return result;
    }

    if (strcmp(argv[1], "decode") == 0 && argc >= 4) {
        const char *label = NULL;
        DecoderBackend backend = parse_backend(argv[2], &label);
        uint64_t max_frames = argc >= 5 ? parse_max_frames(argv[4]) : 0;
        result = decode_once(argv[3], backend, max_frames, -1, &first);
        if (result < 0) {
            free_result(&first);
            return 1;
        }
        print_result_json(label, &first);
        free_result(&first);
        return 0;
    }

    if (strcmp(argv[1], "seek") == 0 && argc >= 5) {
        const char *label = NULL;
        DecoderBackend backend = parse_backend(argv[2], &label);
        int64_t target_us = (int64_t)strtoll(argv[4], NULL, 10);
        uint64_t max_frames = argc >= 6 ? parse_max_frames(argv[5]) : 0;
        result = decode_once(argv[3], backend, max_frames, target_us, &first);
        if (result < 0) {
            free_result(&first);
            return 1;
        }
        print_result_json(label, &first);
        free_result(&first);
        return 0;
    }

    fprintf(stderr, "invalid arguments\n");
    return 64;
}

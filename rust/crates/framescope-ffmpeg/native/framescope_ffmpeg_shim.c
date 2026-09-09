#include <errno.h>
#include <limits.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include <libavcodec/avcodec.h>
#include <libavcodec/packet.h>
#include <libavformat/avformat.h>
#include <libavformat/avio.h>
#include <libavutil/avutil.h>
#include <libavutil/display.h>
#include <libavutil/error.h>
#include <libavutil/pixdesc.h>
#include <libswscale/swscale.h>

#define FS_ERROR_MESSAGE_CAPACITY 256
#define FS_FORMAT_NAME_CAPACITY 128
#define FS_FORMAT_LONG_NAME_CAPACITY 256
#define FS_CODEC_NAME_CAPACITY 64
#define FS_PIXEL_FORMAT_CAPACITY 64
#define FS_IO_BUFFER_SIZE (64 * 1024)

enum FsErrorKind {
    FS_ERR_NONE = 0,
    FS_ERR_UNSUPPORTED_FORMAT = 1,
    FS_ERR_UNSUPPORTED_CODEC = 2,
    FS_ERR_INVALID_SOURCE = 3,
    FS_ERR_NO_VIDEO_STREAM = 4,
    FS_ERR_DECODER = 5,
    FS_ERR_MALFORMED = 6,
    FS_ERR_IO = 7,
    FS_ERR_CANCELLED = 8,
    FS_ERR_SEEK_UNAVAILABLE = 9,
    FS_ERR_BACKEND = 10,
};

enum FsMediaKind {
    FS_MEDIA_UNKNOWN = 0,
    FS_MEDIA_VIDEO = 1,
    FS_MEDIA_AUDIO = 2,
    FS_MEDIA_SUBTITLE = 3,
    FS_MEDIA_DATA = 4,
    FS_MEDIA_ATTACHMENT = 5,
};

typedef int32_t (*FsCancelFn)(void *opaque);

typedef struct FsError {
    int32_t kind;
    int32_t ffmpeg_code;
    char message[FS_ERROR_MESSAGE_CAPACITY];
} FsError;

typedef struct FsContainerInfo {
    char format_name[FS_FORMAT_NAME_CAPACITY];
    char format_long_name[FS_FORMAT_LONG_NAME_CAPACITY];
    int64_t duration_us;
    uint32_t stream_count;
    int32_t selected_stream_index;
} FsContainerInfo;

typedef struct FsStreamInfo {
    int32_t index;
    int32_t media_type;
    int32_t codec_id;
    int32_t decoder_available;
    int32_t is_default;
    char codec_name[FS_CODEC_NAME_CAPACITY];
    int32_t time_base_num;
    int32_t time_base_den;
    int64_t duration_ticks;
    int64_t frame_count;
    int32_t width;
    int32_t height;
    int32_t pixel_format;
    char pixel_format_name[FS_PIXEL_FORMAT_CAPACITY];
    int32_t average_rate_num;
    int32_t average_rate_den;
    int32_t nominal_rate_num;
    int32_t nominal_rate_den;
    int32_t has_rotation;
    int32_t rotation_degrees;
} FsStreamInfo;

typedef struct FsFrameInfo {
    uint64_t epoch;
    uint64_t index;
    int32_t stream_index;
    int32_t has_timestamp;
    int64_t timestamp_ticks;
    int32_t has_duration;
    int64_t duration_ticks;
    int32_t time_base_num;
    int32_t time_base_den;
    int32_t keyframe;
    int32_t corrupt;
    int32_t width;
    int32_t height;
    int32_t pixel_format;
    char pixel_format_name[FS_PIXEL_FORMAT_CAPACITY];
} FsFrameInfo;

typedef struct FsSession FsSession;

typedef struct FsFdInput {
    int fd;
    int seekable;
    int64_t position;
    int64_t size;
    FsSession *owner;
} FsFdInput;

struct FsSession {
    AVFormatContext *format;
    AVCodecContext *codec;
    AVPacket *packet;
    AVFrame *frame;
    struct SwsContext *rgba_scaler;
    AVIOContext *avio;
    FsFdInput *fd_input;
    int32_t selected_stream_index;
    int demux_eof;
    int packet_pending;
    int flush_sent;
    uint64_t epoch;
    uint64_t frame_index;
    FsCancelFn cancel_fn;
    void *cancel_opaque;
};

static void fs_copy_text(char *dst, size_t capacity, const char *src) {
    if (capacity == 0) {
        return;
    }
    if (src == NULL) {
        dst[0] = '\0';
        return;
    }
    (void)snprintf(dst, capacity, "%s", src);
}

static void fs_clear_error(FsError *error) {
    if (error == NULL) {
        return;
    }
    memset(error, 0, sizeof(*error));
}

static void fs_set_error(FsError *error, int32_t kind, int32_t ffmpeg_code, const char *message) {
    if (error == NULL) {
        return;
    }
    memset(error, 0, sizeof(*error));
    error->kind = kind;
    error->ffmpeg_code = ffmpeg_code;
    fs_copy_text(error->message, sizeof(error->message), message);
}

static void fs_set_av_error(FsError *error, int32_t kind, int32_t code, const char *context) {
    char detail[AV_ERROR_MAX_STRING_SIZE];
    char message[FS_ERROR_MESSAGE_CAPACITY];
    if (av_strerror(code, detail, sizeof(detail)) < 0) {
        fs_copy_text(detail, sizeof(detail), "unknown FFmpeg error");
    }
    (void)snprintf(message, sizeof(message), "%s: %s", context, detail);
    fs_set_error(error, kind, code, message);
}

static int fs_is_cancelled(const FsSession *session) {
    if (session == NULL || session->cancel_fn == NULL) {
        return 0;
    }
    return session->cancel_fn(session->cancel_opaque) != 0;
}

static int fs_interrupt_callback(void *opaque) {
    return fs_is_cancelled((const FsSession *)opaque);
}

static int32_t fs_media_kind(enum AVMediaType type) {
    switch (type) {
        case AVMEDIA_TYPE_VIDEO:
            return FS_MEDIA_VIDEO;
        case AVMEDIA_TYPE_AUDIO:
            return FS_MEDIA_AUDIO;
        case AVMEDIA_TYPE_SUBTITLE:
            return FS_MEDIA_SUBTITLE;
        case AVMEDIA_TYPE_DATA:
            return FS_MEDIA_DATA;
        case AVMEDIA_TYPE_ATTACHMENT:
            return FS_MEDIA_ATTACHMENT;
        default:
            return FS_MEDIA_UNKNOWN;
    }
}

static int fs_error_kind_for_open(int code) {
    if (code == AVERROR_EXIT) {
        return FS_ERR_CANCELLED;
    }
    if (code == AVERROR(ENOENT) || code == AVERROR(EACCES) || code == AVERROR(EBADF)) {
        return FS_ERR_INVALID_SOURCE;
    }
    if (code == AVERROR(EIO)) {
        return FS_ERR_IO;
    }
    if (code == AVERROR_INVALIDDATA) {
        return FS_ERR_MALFORMED;
    }
    if (code == AVERROR_DEMUXER_NOT_FOUND || code == AVERROR_PROTOCOL_NOT_FOUND) {
        return FS_ERR_UNSUPPORTED_FORMAT;
    }
    return FS_ERR_UNSUPPORTED_FORMAT;
}

static void fs_session_destroy(FsSession *session) {
    if (session == NULL) {
        return;
    }

    if (session->rgba_scaler != NULL) {
        sws_freeContext(session->rgba_scaler);
        session->rgba_scaler = NULL;
    }
    if (session->codec != NULL) {
        avcodec_free_context(&session->codec);
    }
    if (session->frame != NULL) {
        av_frame_free(&session->frame);
    }
    if (session->packet != NULL) {
        av_packet_free(&session->packet);
    }
    if (session->format != NULL) {
        avformat_close_input(&session->format);
    }
    if (session->avio != NULL) {
        av_freep(&session->avio->buffer);
        avio_context_free(&session->avio);
    }
    if (session->fd_input != NULL) {
        if (session->fd_input->fd >= 0) {
            (void)close(session->fd_input->fd);
        }
        free(session->fd_input);
    }
    free(session);
}

static FsSession *fs_session_alloc(FsCancelFn cancel_fn, void *cancel_opaque, FsError *error) {
    FsSession *session = (FsSession *)calloc(1, sizeof(*session));
    if (session == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(ENOMEM), "failed to allocate decoder session");
        return NULL;
    }
    session->selected_stream_index = -1;
    session->cancel_fn = cancel_fn;
    session->cancel_opaque = cancel_opaque;
    return session;
}

static AVStream *fs_selected_stream(FsSession *session) {
    unsigned int i;
    if (session == NULL || session->format == NULL || session->selected_stream_index < 0) {
        return NULL;
    }
    for (i = 0; i < session->format->nb_streams; ++i) {
        AVStream *stream = session->format->streams[i];
        if (stream != NULL && stream->index == session->selected_stream_index) {
            return stream;
        }
    }
    return NULL;
}

static int fs_select_video_stream(FsSession *session, int32_t requested_stream, FsError *error) {
    AVStream *selected = NULL;
    AVStream *first_supported = NULL;
    AVStream *first_default = NULL;
    int saw_video = 0;
    unsigned int i;

    for (i = 0; i < session->format->nb_streams; ++i) {
        AVStream *stream = session->format->streams[i];
        const AVCodec *decoder;
        if (stream == NULL || stream->codecpar == NULL || stream->codecpar->codec_type != AVMEDIA_TYPE_VIDEO) {
            continue;
        }
        saw_video = 1;
        decoder = avcodec_find_decoder(stream->codecpar->codec_id);

        if (requested_stream >= 0) {
            if (stream->index != requested_stream) {
                continue;
            }
            if (decoder == NULL) {
                char message[FS_ERROR_MESSAGE_CAPACITY];
                (void)snprintf(message, sizeof(message), "no decoder is available for stream %d codec %s", requested_stream, avcodec_get_name(stream->codecpar->codec_id));
                fs_set_error(error, FS_ERR_UNSUPPORTED_CODEC, 0, message);
                return -1;
            }
            selected = stream;
            break;
        }

        if (decoder == NULL) {
            continue;
        }
        if (first_supported == NULL) {
            first_supported = stream;
        }
        if (first_default == NULL && (stream->disposition & AV_DISPOSITION_DEFAULT) != 0) {
            first_default = stream;
        }
    }

    if (requested_stream >= 0 && selected == NULL) {
        fs_set_error(error, FS_ERR_NO_VIDEO_STREAM, 0, "requested stream is absent or is not a video stream");
        return -1;
    }

    if (requested_stream < 0) {
        selected = first_default != NULL ? first_default : first_supported;
    }

    if (selected == NULL) {
        if (saw_video) {
            fs_set_error(error, FS_ERR_UNSUPPORTED_CODEC, 0, "video streams exist, but none use a decoder enabled in this FFmpeg build");
        } else {
            fs_set_error(error, FS_ERR_NO_VIDEO_STREAM, 0, "container contains no video stream");
        }
        return -1;
    }

    session->selected_stream_index = selected->index;
    return 0;
}

static int fs_init_decoder(FsSession *session, FsError *error) {
    AVStream *stream = fs_selected_stream(session);
    const AVCodec *decoder;
    int result;

    if (stream == NULL || stream->codecpar == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "selected video stream disappeared during decoder initialization");
        return -1;
    }

    decoder = avcodec_find_decoder(stream->codecpar->codec_id);
    if (decoder == NULL) {
        fs_set_error(error, FS_ERR_UNSUPPORTED_CODEC, 0, "selected video codec has no enabled decoder");
        return -1;
    }

    session->codec = avcodec_alloc_context3(decoder);
    if (session->codec == NULL) {
        fs_set_error(error, FS_ERR_DECODER, AVERROR(ENOMEM), "failed to allocate video decoder context");
        return -1;
    }

    result = avcodec_parameters_to_context(session->codec, stream->codecpar);
    if (result < 0) {
        fs_set_av_error(error, FS_ERR_DECODER, result, "failed to configure decoder from stream parameters");
        return -1;
    }
    session->codec->pkt_timebase = stream->time_base;

    result = avcodec_open2(session->codec, decoder, NULL);
    if (result < 0) {
        fs_set_av_error(error, FS_ERR_DECODER, result, "failed to open video decoder");
        return -1;
    }

    session->packet = av_packet_alloc();
    session->frame = av_frame_alloc();
    if (session->packet == NULL || session->frame == NULL) {
        fs_set_error(error, FS_ERR_DECODER, AVERROR(ENOMEM), "failed to allocate reusable packet/frame objects");
        return -1;
    }
    return 0;
}

static int fs_finish_open(FsSession *session, int32_t requested_stream, FsError *error) {
    int result;
    if (fs_is_cancelled(session)) {
        fs_set_error(error, FS_ERR_CANCELLED, AVERROR_EXIT, "video open was cancelled");
        return -1;
    }

    result = avformat_find_stream_info(session->format, NULL);
    if (result < 0) {
        if (fs_is_cancelled(session) || result == AVERROR_EXIT) {
            fs_set_error(error, FS_ERR_CANCELLED, result, "stream discovery was cancelled");
        } else {
            fs_set_av_error(error, FS_ERR_MALFORMED, result, "failed to discover media streams");
        }
        return -1;
    }

    if (fs_select_video_stream(session, requested_stream, error) < 0) {
        return -1;
    }
    return fs_init_decoder(session, error);
}

static int fs_fd_read(void *opaque, uint8_t *buffer, int buffer_size) {
    FsFdInput *input = (FsFdInput *)opaque;
    ssize_t result;

    if (input == NULL || input->fd < 0) {
        return AVERROR(EBADF);
    }
    if (fs_is_cancelled(input->owner)) {
        return AVERROR_EXIT;
    }

    if (input->seekable) {
        result = pread(input->fd, buffer, (size_t)buffer_size, (off_t)input->position);
    } else {
        result = read(input->fd, buffer, (size_t)buffer_size);
    }
    if (result < 0) {
        return AVERROR(errno);
    }
    if (result == 0) {
        return AVERROR_EOF;
    }
    input->position += (int64_t)result;
    return (int)result;
}

static int64_t fs_fd_seek(void *opaque, int64_t offset, int whence) {
    FsFdInput *input = (FsFdInput *)opaque;
    int64_t base;
    int64_t target;

    if (input == NULL || input->fd < 0) {
        return AVERROR(EBADF);
    }
    if (fs_is_cancelled(input->owner)) {
        return AVERROR_EXIT;
    }
    if (whence == AVSEEK_SIZE) {
        return input->size >= 0 ? input->size : AVERROR(ENOSYS);
    }
    if (!input->seekable) {
        return AVERROR(ENOSYS);
    }

    whence &= ~AVSEEK_FORCE;
    switch (whence) {
        case SEEK_SET:
            base = 0;
            break;
        case SEEK_CUR:
            base = input->position;
            break;
        case SEEK_END:
            if (input->size < 0) {
                return AVERROR(ENOSYS);
            }
            base = input->size;
            break;
        default:
            return AVERROR(EINVAL);
    }

    if ((offset > 0 && base > INT64_MAX - offset) || (offset < 0 && base < INT64_MIN - offset)) {
        return AVERROR(EOVERFLOW);
    }
    target = base + offset;
    if (target < 0) {
        return AVERROR(EINVAL);
    }
    input->position = target;
    return target;
}

void *framescope_ffmpeg_open_path(const char *path, int32_t requested_stream, FsCancelFn cancel_fn, void *cancel_opaque, FsError *error) {
    FsSession *session;
    int result;

    fs_clear_error(error);
    if (path == NULL || path[0] == '\0') {
        fs_set_error(error, FS_ERR_INVALID_SOURCE, 0, "video path is empty");
        return NULL;
    }

    session = fs_session_alloc(cancel_fn, cancel_opaque, error);
    if (session == NULL) {
        return NULL;
    }
    if (fs_is_cancelled(session)) {
        fs_set_error(error, FS_ERR_CANCELLED, AVERROR_EXIT, "video open was cancelled");
        fs_session_destroy(session);
        return NULL;
    }

    session->format = avformat_alloc_context();
    if (session->format == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(ENOMEM), "failed to allocate format context");
        fs_session_destroy(session);
        return NULL;
    }
    session->format->interrupt_callback.callback = fs_interrupt_callback;
    session->format->interrupt_callback.opaque = session;

    result = avformat_open_input(&session->format, path, NULL, NULL);
    if (result < 0) {
        int32_t kind = fs_is_cancelled(session) ? FS_ERR_CANCELLED : fs_error_kind_for_open(result);
        fs_set_av_error(error, kind, result, "failed to open video source");
        fs_session_destroy(session);
        return NULL;
    }

    if (fs_finish_open(session, requested_stream, error) < 0) {
        fs_session_destroy(session);
        return NULL;
    }
    return session;
}

void *framescope_ffmpeg_open_fd(int32_t fd, int32_t requested_stream, FsCancelFn cancel_fn, void *cancel_opaque, FsError *error) {
    FsSession *session;
    FsFdInput *input;
    uint8_t *buffer;
    struct stat stat_buffer;
    off_t current;
    int duplicate;
    int result;

    fs_clear_error(error);
    if (fd < 0) {
        fs_set_error(error, FS_ERR_INVALID_SOURCE, 0, "file descriptor is negative");
        return NULL;
    }

    duplicate = dup(fd);
    if (duplicate < 0) {
        fs_set_error(error, FS_ERR_INVALID_SOURCE, AVERROR(errno), "failed to duplicate video file descriptor");
        return NULL;
    }

    session = fs_session_alloc(cancel_fn, cancel_opaque, error);
    if (session == NULL) {
        (void)close(duplicate);
        return NULL;
    }

    input = (FsFdInput *)calloc(1, sizeof(*input));
    if (input == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(ENOMEM), "failed to allocate file-descriptor input state");
        (void)close(duplicate);
        fs_session_destroy(session);
        return NULL;
    }
    input->fd = duplicate;
    input->size = -1;
    input->owner = session;
    session->fd_input = input;

    current = lseek(duplicate, 0, SEEK_CUR);
    if (current >= 0) {
        input->seekable = 1;
        input->position = (int64_t)current;
    } else if (errno == ESPIPE) {
        input->seekable = 0;
        input->position = 0;
    } else {
        fs_set_error(error, FS_ERR_IO, AVERROR(errno), "failed to inspect video file-descriptor position");
        fs_session_destroy(session);
        return NULL;
    }

    if (fstat(duplicate, &stat_buffer) == 0 && S_ISREG(stat_buffer.st_mode)) {
        input->size = (int64_t)stat_buffer.st_size;
    }

    buffer = (uint8_t *)av_malloc(FS_IO_BUFFER_SIZE);
    if (buffer == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(ENOMEM), "failed to allocate FFmpeg I/O buffer");
        fs_session_destroy(session);
        return NULL;
    }

    session->avio = avio_alloc_context(
        buffer,
        FS_IO_BUFFER_SIZE,
        0,
        input,
        fs_fd_read,
        NULL,
        input->seekable ? fs_fd_seek : NULL
    );
    if (session->avio == NULL) {
        av_free(buffer);
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(ENOMEM), "failed to allocate FFmpeg I/O context");
        fs_session_destroy(session);
        return NULL;
    }
    session->avio->seekable = input->seekable ? AVIO_SEEKABLE_NORMAL : 0;

    session->format = avformat_alloc_context();
    if (session->format == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(ENOMEM), "failed to allocate format context");
        fs_session_destroy(session);
        return NULL;
    }
    session->format->pb = session->avio;
    session->format->flags |= AVFMT_FLAG_CUSTOM_IO;
    session->format->interrupt_callback.callback = fs_interrupt_callback;
    session->format->interrupt_callback.opaque = session;

    result = avformat_open_input(&session->format, NULL, NULL, NULL);
    if (result < 0) {
        int32_t kind = fs_is_cancelled(session) ? FS_ERR_CANCELLED : fs_error_kind_for_open(result);
        fs_set_av_error(error, kind, result, "failed to open file-descriptor video source");
        fs_session_destroy(session);
        return NULL;
    }

    if (fs_finish_open(session, requested_stream, error) < 0) {
        fs_session_destroy(session);
        return NULL;
    }
    return session;
}

void framescope_ffmpeg_close(void *opaque) {
    fs_session_destroy((FsSession *)opaque);
}

int32_t framescope_ffmpeg_container_info(void *opaque, FsContainerInfo *out, FsError *error) {
    FsSession *session = (FsSession *)opaque;
    fs_clear_error(error);
    if (session == NULL || session->format == NULL || out == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "decoder session is not open");
        return -1;
    }

    memset(out, 0, sizeof(*out));
    out->duration_us = -1;
    out->stream_count = session->format->nb_streams;
    out->selected_stream_index = session->selected_stream_index;
    if (session->format->iformat != NULL) {
        fs_copy_text(out->format_name, sizeof(out->format_name), session->format->iformat->name);
        fs_copy_text(out->format_long_name, sizeof(out->format_long_name), session->format->iformat->long_name);
    }
    if (session->format->duration != AV_NOPTS_VALUE && session->format->duration >= 0) {
        out->duration_us = session->format->duration;
    }
    return 0;
}

static int32_t fs_stream_rotation(const AVStream *stream, int32_t *rotation) {
    const AVPacketSideData *side_data;
    double angle;
    long rounded;
    int32_t normalized;

    if (stream == NULL || stream->codecpar == NULL || rotation == NULL) {
        return 0;
    }
    side_data = av_packet_side_data_get(
        stream->codecpar->coded_side_data,
        stream->codecpar->nb_coded_side_data,
        AV_PKT_DATA_DISPLAYMATRIX
    );
    if (side_data == NULL || side_data->data == NULL || side_data->size < 9U * sizeof(int32_t)) {
        return 0;
    }

    angle = av_display_rotation_get((const int32_t *)side_data->data);
    if (isnan(angle)) {
        return 0;
    }
    rounded = lround(angle);
    normalized = (int32_t)(rounded % 360L);
    if (normalized < 0) {
        normalized += 360;
    }
    *rotation = normalized;
    return 1;
}

int32_t framescope_ffmpeg_stream_info(void *opaque, uint32_t ordinal, FsStreamInfo *out, FsError *error) {
    FsSession *session = (FsSession *)opaque;
    AVStream *stream;
    AVCodecParameters *parameters;
    AVRational nominal_rate;
    const char *pixel_name;
    int32_t rotation = 0;

    fs_clear_error(error);
    if (session == NULL || session->format == NULL || out == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "decoder session is not open");
        return -1;
    }
    if (ordinal >= session->format->nb_streams) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "stream ordinal is out of range");
        return -1;
    }

    stream = session->format->streams[ordinal];
    if (stream == NULL || stream->codecpar == NULL) {
        fs_set_error(error, FS_ERR_MALFORMED, 0, "container exposes an invalid stream entry");
        return -1;
    }
    parameters = stream->codecpar;

    memset(out, 0, sizeof(*out));
    out->index = stream->index;
    out->media_type = fs_media_kind(parameters->codec_type);
    out->codec_id = (int32_t)parameters->codec_id;
    out->decoder_available = avcodec_find_decoder(parameters->codec_id) != NULL;
    out->is_default = (stream->disposition & AV_DISPOSITION_DEFAULT) != 0;
    fs_copy_text(out->codec_name, sizeof(out->codec_name), avcodec_get_name(parameters->codec_id));
    out->time_base_num = stream->time_base.num;
    out->time_base_den = stream->time_base.den;
    out->duration_ticks = stream->duration;
    out->frame_count = stream->nb_frames;
    out->width = parameters->width;
    out->height = parameters->height;
    out->pixel_format = parameters->format;
    pixel_name = parameters->format >= 0 ? av_get_pix_fmt_name((enum AVPixelFormat)parameters->format) : NULL;
    fs_copy_text(out->pixel_format_name, sizeof(out->pixel_format_name), pixel_name);
    out->average_rate_num = stream->avg_frame_rate.num;
    out->average_rate_den = stream->avg_frame_rate.den;

    nominal_rate = parameters->framerate;
    if (nominal_rate.num <= 0 || nominal_rate.den <= 0) {
        nominal_rate = av_guess_frame_rate(session->format, stream, NULL);
    }
    out->nominal_rate_num = nominal_rate.num;
    out->nominal_rate_den = nominal_rate.den;
    out->has_rotation = fs_stream_rotation(stream, &rotation);
    out->rotation_degrees = rotation;
    return 0;
}

static void fs_fill_frame_info(FsSession *session, AVStream *stream, FsFrameInfo *out) {
    int64_t timestamp;
    const char *pixel_name;

    memset(out, 0, sizeof(*out));
    out->epoch = session->epoch;
    out->index = session->frame_index++;
    out->stream_index = session->selected_stream_index;
    out->time_base_num = stream->time_base.num;
    out->time_base_den = stream->time_base.den;

    timestamp = session->frame->best_effort_timestamp;
    if (timestamp == AV_NOPTS_VALUE) {
        timestamp = session->frame->pts;
    }
    if (timestamp != AV_NOPTS_VALUE) {
        out->has_timestamp = 1;
        out->timestamp_ticks = timestamp;
    }
    if (session->frame->duration > 0) {
        out->has_duration = 1;
        out->duration_ticks = session->frame->duration;
    }
    out->keyframe = (session->frame->flags & AV_FRAME_FLAG_KEY) != 0;
    out->corrupt = ((session->frame->flags & AV_FRAME_FLAG_CORRUPT) != 0) || session->frame->decode_error_flags != 0;
    out->width = session->frame->width;
    out->height = session->frame->height;
    out->pixel_format = session->frame->format;
    pixel_name = session->frame->format >= 0 ? av_get_pix_fmt_name((enum AVPixelFormat)session->frame->format) : NULL;
    fs_copy_text(out->pixel_format_name, sizeof(out->pixel_format_name), pixel_name);
}

static int fs_submit_current_packet(FsSession *session, FsError *error) {
    int result = avcodec_send_packet(session->codec, session->packet);
    if (result == 0) {
        av_packet_unref(session->packet);
        session->packet_pending = 0;
        return 1;
    }
    if (result == AVERROR(EAGAIN)) {
        session->packet_pending = 1;
        return 0;
    }

    av_packet_unref(session->packet);
    session->packet_pending = 0;
    if (result == AVERROR_INVALIDDATA) {
        fs_set_av_error(error, FS_ERR_MALFORMED, result, "decoder rejected malformed packet data");
    } else {
        fs_set_av_error(error, FS_ERR_DECODER, result, "failed to submit packet to video decoder");
    }
    return -1;
}

int32_t framescope_ffmpeg_next_frame(void *opaque, FsFrameInfo *out, FsError *error) {
    FsSession *session = (FsSession *)opaque;
    AVStream *stream;
    int result;

    fs_clear_error(error);
    if (session == NULL || session->format == NULL || session->codec == NULL || session->packet == NULL || session->frame == NULL || out == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "decoder session is not initialized");
        return -1;
    }
    stream = fs_selected_stream(session);
    if (stream == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "selected stream is unavailable");
        return -1;
    }

    for (;;) {
        if (fs_is_cancelled(session)) {
            fs_set_error(error, FS_ERR_CANCELLED, AVERROR_EXIT, "video decoding was cancelled");
            return -1;
        }

        av_frame_unref(session->frame);
        result = avcodec_receive_frame(session->codec, session->frame);
        if (result == 0) {
            fs_fill_frame_info(session, stream, out);
            return 1;
        }
        if (result == AVERROR_EOF) {
            return 0;
        }
        if (result != AVERROR(EAGAIN)) {
            int32_t kind = result == AVERROR_INVALIDDATA ? FS_ERR_MALFORMED : FS_ERR_DECODER;
            fs_set_av_error(error, kind, result, "failed to receive a decoded video frame");
            return -1;
        }

        if (session->flush_sent) {
            fs_set_error(error, FS_ERR_DECODER, AVERROR(EAGAIN), "decoder requested more packets after end-of-stream flush");
            return -1;
        }

        for (;;) {
            if (session->packet_pending) {
                result = fs_submit_current_packet(session, error);
                if (result < 0) {
                    return -1;
                }
                break;
            }

            if (session->demux_eof) {
                result = avcodec_send_packet(session->codec, NULL);
                if (result == 0) {
                    session->flush_sent = 1;
                    break;
                }
                if (result == AVERROR(EAGAIN)) {
                    break;
                }
                if (result == AVERROR_EOF) {
                    session->flush_sent = 1;
                    return 0;
                }
                fs_set_av_error(error, FS_ERR_DECODER, result, "failed to flush video decoder at end of stream");
                return -1;
            }

            av_packet_unref(session->packet);
            result = av_read_frame(session->format, session->packet);
            if (result == AVERROR_EOF) {
                session->demux_eof = 1;
                continue;
            }
            if (result < 0) {
                if (fs_is_cancelled(session) || result == AVERROR_EXIT) {
                    fs_set_error(error, FS_ERR_CANCELLED, result, "video decoding was cancelled while reading packets");
                } else {
                    int32_t kind = result == AVERROR_INVALIDDATA ? FS_ERR_MALFORMED : FS_ERR_IO;
                    fs_set_av_error(error, kind, result, "failed while reading media packets");
                }
                return -1;
            }

            if (session->packet->stream_index != session->selected_stream_index) {
                continue;
            }

            result = fs_submit_current_packet(session, error);
            if (result < 0) {
                return -1;
            }
            break;
        }
    }
}

static int32_t fs_copy_current_frame_rgba_to(
    FsSession *session,
    int32_t target_width,
    int32_t target_height,
    int flags,
    uint8_t *output,
    size_t output_capacity,
    int32_t *out_stride,
    FsError *error
) {
    uint8_t *destination_data[4] = {NULL, NULL, NULL, NULL};
    int destination_linesize[4] = {0, 0, 0, 0};
    size_t stride;
    size_t required;
    int scaled_rows;

    fs_clear_error(error);
    if (session == NULL || session->frame == NULL || output == NULL || out_stride == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "decoded frame RGBA copy received invalid arguments");
        return -1;
    }
    if (session->frame->width <= 0 || session->frame->height <= 0 || session->frame->format < 0 || session->frame->data[0] == NULL) {
        fs_set_error(error, FS_ERR_DECODER, 0, "no valid decoded frame is available for RGBA copy");
        return -1;
    }
    if (target_width <= 0 || target_height <= 0 ||
        target_width > session->frame->width || target_height > session->frame->height) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(EINVAL), "RGBA target dimensions must be positive and must not upscale the decoded frame");
        return -1;
    }
    if (target_width > INT_MAX / 4) {
        fs_set_error(error, FS_ERR_DECODER, AVERROR(EOVERFLOW), "RGBA target width overflows stride");
        return -1;
    }

    stride = (size_t)target_width * 4U;
    if ((size_t)target_height > SIZE_MAX / stride) {
        fs_set_error(error, FS_ERR_DECODER, AVERROR(EOVERFLOW), "RGBA target dimensions overflow buffer size");
        return -1;
    }
    required = stride * (size_t)target_height;
    if (output_capacity < required) {
        fs_set_error(error, FS_ERR_BACKEND, AVERROR(ENOSPC), "RGBA output buffer is smaller than the requested target frame");
        return -1;
    }

    session->rgba_scaler = sws_getCachedContext(
        session->rgba_scaler,
        session->frame->width,
        session->frame->height,
        (enum AVPixelFormat)session->frame->format,
        target_width,
        target_height,
        AV_PIX_FMT_RGBA,
        flags,
        NULL,
        NULL,
        NULL
    );
    if (session->rgba_scaler == NULL) {
        fs_set_error(error, FS_ERR_DECODER, AVERROR(ENOMEM), "failed to create or reuse RGBA conversion context");
        return -1;
    }

    destination_data[0] = output;
    destination_linesize[0] = (int)stride;
    scaled_rows = sws_scale(
        session->rgba_scaler,
        (const uint8_t *const *)session->frame->data,
        session->frame->linesize,
        0,
        session->frame->height,
        destination_data,
        destination_linesize
    );

    if (scaled_rows != target_height) {
        fs_set_error(error, FS_ERR_DECODER, 0, "FFmpeg did not convert the complete decoded frame to the requested RGBA size");
        return -1;
    }

    *out_stride = (int32_t)stride;
    return 0;
}

int32_t framescope_ffmpeg_copy_current_frame_rgba(
    void *opaque,
    uint8_t *output,
    size_t output_capacity,
    int32_t *out_stride,
    FsError *error
) {
    FsSession *session = (FsSession *)opaque;
    if (session == NULL || session->frame == NULL) {
        return fs_copy_current_frame_rgba_to(
            session, 0, 0, SWS_BILINEAR, output, output_capacity, out_stride, error
        );
    }
    return fs_copy_current_frame_rgba_to(
        session,
        session->frame->width,
        session->frame->height,
        SWS_BILINEAR,
        output,
        output_capacity,
        out_stride,
        error
    );
}

int32_t framescope_ffmpeg_copy_current_frame_rgba_scaled(
    void *opaque,
    int32_t target_width,
    int32_t target_height,
    uint8_t *output,
    size_t output_capacity,
    int32_t *out_stride,
    FsError *error
) {
    return fs_copy_current_frame_rgba_to(
        (FsSession *)opaque,
        target_width,
        target_height,
        SWS_POINT,
        output,
        output_capacity,
        out_stride,
        error
    );
}

int32_t framescope_ffmpeg_seek_us(void *opaque, int64_t timestamp_us, FsError *error) {
    FsSession *session = (FsSession *)opaque;
    AVStream *stream;
    int64_t target;
    int result;

    fs_clear_error(error);
    if (session == NULL || session->format == NULL || session->codec == NULL) {
        fs_set_error(error, FS_ERR_BACKEND, 0, "decoder session is not initialized");
        return -1;
    }
    if (timestamp_us < 0) {
        fs_set_error(error, FS_ERR_SEEK_UNAVAILABLE, AVERROR(EINVAL), "seek timestamp must be non-negative");
        return -1;
    }
    if (fs_is_cancelled(session)) {
        fs_set_error(error, FS_ERR_CANCELLED, AVERROR_EXIT, "video seek was cancelled");
        return -1;
    }

    stream = fs_selected_stream(session);
    if (stream == NULL || stream->time_base.num <= 0 || stream->time_base.den <= 0) {
        fs_set_error(error, FS_ERR_SEEK_UNAVAILABLE, 0, "selected stream has no valid seek time base");
        return -1;
    }
    target = av_rescale_q(timestamp_us, AV_TIME_BASE_Q, stream->time_base);
    result = avformat_seek_file(
        session->format,
        session->selected_stream_index,
        INT64_MIN,
        target,
        target,
        AVSEEK_FLAG_BACKWARD
    );
    if (result < 0) {
        if (fs_is_cancelled(session) || result == AVERROR_EXIT) {
            fs_set_error(error, FS_ERR_CANCELLED, result, "video seek was cancelled");
        } else {
            fs_set_av_error(error, FS_ERR_SEEK_UNAVAILABLE, result, "FFmpeg could not seek this source");
        }
        return -1;
    }

    avcodec_flush_buffers(session->codec);
    if (session->packet != NULL) {
        av_packet_unref(session->packet);
    }
    if (session->frame != NULL) {
        av_frame_unref(session->frame);
    }
    session->demux_eof = 0;
    session->packet_pending = 0;
    session->flush_sent = 0;
    session->epoch += 1;
    session->frame_index = 0;
    return 0;
}

/*
 * ring_ffi: C ABI over Silicon Ring call media (crates/ring-ffi, built as ring_ffi.dll).
 *
 * Joins a call's LiveKit room with the platform audio stack (WASAPI with WebRTC echo
 * cancellation, noise suppression and gain control), publishes one microphone track and plays
 * every remote audio track. The Ring API hands out the room URL and token as a MediaGrant
 * (POST /v1/calls, /accept, /media); everything else about the call stays in the app.
 *
 * Threading: every function may be called from any thread. Events are delivered on one
 * library-owned background thread, in order. Do not call ring_media_disconnect for a handle from
 * inside that handle's callback unless you are done with it; it is allowed but ends delivery.
 *
 * Memory: strings returned as `char *` belong to the caller and are released with
 * ring_string_free. Strings passed in are only read during the call.
 *
 * Errors: functions returning int32_t return RING_OK (0) or a negative RING_ERR_* code; functions
 * returning pointers return NULL. Either way ring_ffi_last_error() describes the failure on the
 * calling thread.
 */
#ifndef RING_FFI_H
#define RING_FFI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define RING_OK 0
#define RING_ERR_INVALID_ARGUMENT (-1)
#define RING_ERR_MEDIA (-2)
#define RING_ERR_INTERNAL (-3)

typedef struct RingMedia RingMedia;

/*
 * Receives one event as UTF-8 JSON. `event_json` is valid only during the call. One of:
 *
 *   {"type":"state","state":"connecting"|"connected"|"reconnecting"|"disconnected","reason":string|null}
 *   {"type":"participant_joined","identity":"carbon:<uuid>:dev_…"}
 *   {"type":"participant_left","identity":"…"}
 *   {"type":"active_speakers","identities":["…"]}      loudest first, includes the local identity
 *   {"type":"local_level","level":0.0-1.0}               about 10 Hz
 *   {"type":"remote_muted","identity":"…","muted":true}
 *
 * Identities match Participant.media_identity in the Ring API. Ignore unknown types.
 * "disconnected" is final for the handle; call ring_media_disconnect to free it.
 */
typedef void (*ring_media_event_cb)(void *ctx, const char *event_json);

/* Library version, e.g. "1.0.0". Static storage; do not free. */
const char *ring_ffi_version(void);

/* The last error on the calling thread, or NULL. Free with ring_string_free. */
char *ring_ffi_last_error(void);

/* Frees a string returned by this library. NULL is ignored. */
void ring_string_free(char *s);

/*
 * Starts joining the room and returns at once. Returns NULL only for invalid arguments.
 *
 *   url, token     MediaGrant.url and MediaGrant.token (required)
 *   options_json   NULL, or {"input_device":id|null,"output_device":id|null,"muted":bool}
 *                  (device ids from the *_devices_json lists; null selects the system default)
 *   callback, ctx  must stay valid until ring_media_disconnect returns for this handle
 */
RingMedia *ring_media_connect(const char *url, const char *token, const char *options_json,
                              ring_media_event_cb callback, void *ctx);

/* Mutes (non-zero) or unmutes (0) the microphone. Works while connecting. */
int32_t ring_media_set_muted(RingMedia *media, int32_t muted);

/* 1 when muted, 0 when live, a negative RING_ERR_* code on error. */
int32_t ring_media_is_muted(RingMedia *media);

/* Switches the microphone / speaker. NULL or "" selects the system default. */
int32_t ring_media_set_input_device(RingMedia *media, const char *device_id);
int32_t ring_media_set_output_device(RingMedia *media, const char *device_id);

/*
 * Leaves the room, releases the audio devices and frees the handle. Blocks for up to about
 * 5 seconds, so call it off the UI thread. No callback runs for the handle after it returns.
 * NULL is ignored.
 */
void ring_media_disconnect(RingMedia *media);

/*
 * Audio devices as a JSON array: [{"id":"…","name":"…","is_default":true}, …].
 * NULL on failure. Free with ring_string_free.
 */
char *ring_media_input_devices_json(void);
char *ring_media_output_devices_json(void);

#ifdef __cplusplus
}
#endif

#endif /* RING_FFI_H */

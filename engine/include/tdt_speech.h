/* The speech path of TDT for Android: the take path of TDT desktop on Trit.
 * C ABI of the tdt-speech library (engine/src/ffi.rs).
 *
 * Rules for every function:
 * - Strings are UTF-8 and NUL-terminated.
 * - A call that fails returns NULL or -1. tdt_last_error() then gives the
 *   reason, for the calling thread.
 * - An engine lives until the process ends. It is safe to use from several
 *   threads.
 * - A take is safe to use from several threads until tdt_take_finish() or
 *   tdt_take_cancel() gets it. Those two free it: no call can use it after.
 */
#ifndef TDT_SPEECH_H
#define TDT_SPEECH_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TdtEngine TdtEngine;
typedef struct TdtTake TdtTake;

/* The error of the last failed call on this thread, or "". Valid until the
 * next call on this thread; do not free. */
const char *tdt_last_error(void);

/* The Trit version built into this library, for example "0.11.0". Static. */
const char *tdt_trit_version(void);

/* An engine for the model in `model_dir`. Nothing loads yet. `precision` is
 * how Trit holds the weights: "auto", "int8", "f16", "f32", or "q8_0".
 * `end_quiet_ms` is the digital quiet that the model gets after the audio
 * of each read that ends a take: 480 for Parakeet Unified English, which
 * writes a word about 0.3 s after the word ends, and 0 for Parakeet Redux. */
TdtEngine *tdt_engine_new(const char *model_dir, const char *precision, int end_quiet_ms);

/* Loads the model when it is not in memory. Returns 0, or -1. */
int tdt_engine_load(const TdtEngine *engine);

/* Frees the model. A take in work keeps its model until it ends. */
void tdt_engine_unload(const TdtEngine *engine);

/* Terms to favor, one per line. "" clears them. The next take applies
 * them. Returns 0, or -1. */
int tdt_engine_set_dictionary(const TdtEngine *engine, const char *terms);

/* Starts a take. The model loads on the take's thread when it is not in
 * memory, and reads the audio while it comes. */
TdtTake *tdt_take_start(const TdtEngine *engine);

/* Audio from the microphone: 16 kHz mono PCM16. Returns 0 when the take
 * reached its 120 s limit or tdt_take_stop() was called, else 1. */
int tdt_take_push(const TdtTake *take, const int16_t *samples, size_t count);

/* Stops accepting audio. Returns 0 to tell the recorder to stop. */
int tdt_take_stop(const TdtTake *take);

/* Ends the take, waits for its text, and frees the take. Returns the text
 * ("" for no speech), or NULL on failure. Free it with tdt_text_free(). */
char *tdt_take_finish(TdtTake *take);

/* Drops the take and frees it. NULL is a no-op. */
void tdt_take_cancel(TdtTake *take);

/* Frees text from tdt_take_finish(). NULL is a no-op. */
void tdt_text_free(char *text);

#ifdef __cplusplus
}
#endif

#endif /* TDT_SPEECH_H */

// JNI bridge between TritEngine.kt and the C ABI of the speech library (engine/include/tdt_speech.h).
//
// A failed call returns NULL or -1; the bridge then throws IllegalStateException with tdt_last_error(), which
// names the cause.

#include <jni.h>

#include <cstring>

#include "tdt_speech.h"

namespace {

void ThrowLastError(JNIEnv* env) {
  const char* message = tdt_last_error();
  env->ThrowNew(env->FindClass("java/lang/IllegalStateException"),
                message && *message ? message : "The speech engine failed.");
}

// Standard UTF-8 bytes: NewStringUTF expects modified UTF-8 and breaks on characters outside the BMP.
jbyteArray Utf8(JNIEnv* env, const char* text) {
  const jsize length = static_cast<jsize>(strlen(text));
  jbyteArray out = env->NewByteArray(length);
  if (out && length) env->SetByteArrayRegion(out, 0, length, reinterpret_cast<const jbyte*>(text));
  return out;
}

// A Java string as UTF-8 bytes from Kotlin (encodeToByteArray), NUL-terminated for the library.
struct CString {
  CString(JNIEnv* env, jbyteArray bytes) {
    const jsize length = env->GetArrayLength(bytes);
    data = new char[length + 1];
    env->GetByteArrayRegion(bytes, 0, length, reinterpret_cast<jbyte*>(data));
    data[length] = '\0';
  }
  ~CString() { delete[] data; }
  CString(const CString&) = delete;
  CString& operator=(const CString&) = delete;
  char* data;
};

const TdtEngine* Engine(jlong handle) { return reinterpret_cast<const TdtEngine*>(handle); }
TdtTake* Take(jlong handle) { return reinterpret_cast<TdtTake*>(handle); }

// The recorder gives 20 ms chunks (320 samples); a longer one goes in parts.
constexpr jsize kChunk = 1600;

}  // namespace

extern "C" JNIEXPORT jlong JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeNew(JNIEnv* env, jclass, jbyteArray model_dir,
                                                      jbyteArray precision, jint end_quiet_ms) {
  CString dir(env, model_dir);
  CString name(env, precision);
  TdtEngine* engine = tdt_engine_new(dir.data, name.data, end_quiet_ms);
  if (!engine) ThrowLastError(env);
  return reinterpret_cast<jlong>(engine);
}

extern "C" JNIEXPORT void JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeLoad(JNIEnv* env, jclass, jlong engine) {
  if (tdt_engine_load(Engine(engine)) != 0) ThrowLastError(env);
}

extern "C" JNIEXPORT void JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeUnload(JNIEnv*, jclass, jlong engine) {
  tdt_engine_unload(Engine(engine));
}

extern "C" JNIEXPORT void JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeSetDictionary(JNIEnv* env, jclass, jlong engine,
                                                                jbyteArray terms) {
  CString lines(env, terms);
  if (tdt_engine_set_dictionary(Engine(engine), lines.data) != 0) ThrowLastError(env);
}

extern "C" JNIEXPORT jlong JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeTakeStart(JNIEnv* env, jclass, jlong engine) {
  TdtTake* take = tdt_take_start(Engine(engine));
  if (!take) ThrowLastError(env);
  return reinterpret_cast<jlong>(take);
}

// Returns false when the take needs no more audio.
extern "C" JNIEXPORT jboolean JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeTakePush(JNIEnv* env, jclass, jlong take, jshortArray chunk,
                                                           jint offset, jint length) {
  jshort samples[kChunk];
  bool more = true;
  while (length > 0 && more) {
    const jsize part = length < kChunk ? length : kChunk;
    env->GetShortArrayRegion(chunk, offset, part, samples);
    if (env->ExceptionCheck()) return JNI_FALSE;
    more = tdt_take_push(Take(take), samples, static_cast<size_t>(part)) != 0;
    offset += part;
    length -= part;
  }
  return more ? JNI_TRUE : JNI_FALSE;
}

// Returns true while the last word still ends.
extern "C" JNIEXPORT jboolean JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeTakeStop(JNIEnv*, jclass, jlong take) {
  return tdt_take_stop(Take(take)) != 0 ? JNI_TRUE : JNI_FALSE;
}

// Frees the take.
extern "C" JNIEXPORT jbyteArray JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeTakeFinish(JNIEnv* env, jclass, jlong take) {
  char* text = tdt_take_finish(Take(take));
  if (!text) {
    ThrowLastError(env);
    return nullptr;
  }
  jbyteArray out = Utf8(env, text);
  tdt_text_free(text);
  return out;
}

// Frees the take.
extern "C" JNIEXPORT void JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeTakeCancel(JNIEnv*, jclass, jlong take) {
  tdt_take_cancel(Take(take));
}

// The Trit version built into this app, for example "0.11.0".
extern "C" JNIEXPORT jbyteArray JNICALL
Java_io_github_hi9841_tdt_engine_TritEngine_nativeVersion(JNIEnv* env, jclass) {
  return Utf8(env, tdt_trit_version());
}

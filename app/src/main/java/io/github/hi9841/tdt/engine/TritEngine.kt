package io.github.hi9841.tdt.engine

import io.github.hi9841.tdt.model.ModelStore
import java.util.concurrent.Executors
import kotlin.coroutines.EmptyCoroutineContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.withContext

/**
 * A Trit model over JNI: Parakeet Redux (ternary) or Parakeet Unified English (Q8_0). The native side
 * (engine/, a Rust library) is the take path of TDT desktop: the model reads a take while the user talks, so the
 * text is ready soon after the stop. Trit runs on the phone's fast cores with NEON, and the Armv8.2 dot product
 * where the CPU has it.
 * See docs/adr/0003-run-the-take-path-of-tdt-desktop.md.
 */
class TritEngine(private val store: ModelStore, override val model: SpeechModel) : SpeechEngine {

    // One thread loads and frees the model, so the thread that draws never waits for either.
    private val dispatcher = Executors.newSingleThreadExecutor { Thread(it, "tdt-engine") }.asCoroutineDispatcher()

    /** The native engine. It holds no model until the first load or take, and lives as long as the app. */
    private val handle: Long by lazy {
        nativeNew(
            store.dir.absolutePath.encodeToByteArray(),
            store.pack.precision.encodeToByteArray(),
            store.pack.endQuietMillis,
        )
    }

    // Trit maps the weights from a file, so Android can drop their pages when memory is short and read them again.
    override val staysLoaded get() = true

    /** The terms the native engine has. */
    private var dictionary = emptyList<String>()

    // Three file sizes: cheap enough for the main thread.
    override suspend fun isInstalled() = store.isInstalled()

    override suspend fun download(progress: (Float?) -> Unit) =
        store.download { done -> progress(done.toFloat() / store.pack.totalBytes) }

    override suspend fun load() = withContext(dispatcher) {
        check(store.isInstalled()) { "Download the speech model first." }
        nativeLoad(handle)
    }

    override fun setDictionary(terms: List<String>) {
        if (terms == dictionary) return
        dictionary = terms
        nativeSetDictionary(handle, terms.joinToString("\n").encodeToByteArray())
    }

    override fun open(): Take = NativeTake(nativeTakeStart(handle))

    override fun unload() {
        dispatcher.dispatch(EmptyCoroutineContext) { nativeUnload(handle) }
    }

    /**
     * A take of the native engine. [finish] and [cancel] free the native take. The recorder thread can still be
     * in [push] then, so the three share a lock, and a push after the end does nothing.
     */
    private class NativeTake(private var take: Long) : Take {
        @Synchronized
        override fun push(chunk: ShortArray, offset: Int, length: Int): Boolean =
            take != 0L && nativeTakePush(take, chunk, offset, length)

        @Synchronized
        override fun stop(): Boolean = take != 0L && nativeTakeStop(take)

        override suspend fun finish(): String {
            val native = detach()
            check(native != 0L) { "The take has ended." }
            return withContext(Dispatchers.IO) { nativeTakeFinish(native).decodeToString().trim() }
        }

        override fun cancel() {
            val native = detach()
            if (native != 0L) nativeTakeCancel(native)
        }

        @Synchronized
        private fun detach(): Long = take.also { take = 0L }
    }

    companion object {
        init {
            System.loadLibrary("tdt_engine")
        }

        /** The Trit version built into the app, for example "0.11.0". */
        val version: String by lazy { nativeVersion().decodeToString() }

        @JvmStatic private external fun nativeVersion(): ByteArray
        @JvmStatic private external fun nativeNew(modelDir: ByteArray, precision: ByteArray, endQuietMillis: Int): Long
        @JvmStatic private external fun nativeLoad(engine: Long)
        @JvmStatic private external fun nativeUnload(engine: Long)
        @JvmStatic private external fun nativeSetDictionary(engine: Long, terms: ByteArray)
        @JvmStatic private external fun nativeTakeStart(engine: Long): Long
        @JvmStatic private external fun nativeTakePush(take: Long, chunk: ShortArray, offset: Int, length: Int): Boolean
        @JvmStatic private external fun nativeTakeStop(take: Long): Boolean
        @JvmStatic private external fun nativeTakeFinish(take: Long): ByteArray
        @JvmStatic private external fun nativeTakeCancel(take: Long)
    }
}

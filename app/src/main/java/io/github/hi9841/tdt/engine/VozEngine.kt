package io.github.hi9841.tdt.engine

import ai.desertant.voz.Voz
import android.content.Context
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * Voz by Desert Ant Labs (ai.desertant:voz), on LiteRT. The SDK downloads and checks its own files; they live in
 * app storage, not the cache, so Android does not delete them when space runs low. The SDK reads a whole take in
 * one call, so the text comes after the stop.
 */
class VozEngine(private val context: Context) : SpeechEngine {
    override val model = SpeechModel.Voz

    private val dir = File(context.filesDir, "models/voz")
    private val lock = Mutex()
    private var voz: Voz? = null
    @Volatile private var warm = false
    // The files stay once they are there, so a yes is kept.
    @Volatile private var installed = false

    // isDownloaded() verifies all 1.25 GB of files (3.6 s on the emulator) and waits while a take runs, so it
    // runs on the IO pool, once per process.
    override suspend fun isInstalled(): Boolean = installed ||
        withContext(Dispatchers.IO) { recognizer().isDownloaded() }.also { installed = it }

    // The Kotlin SDK reports no download progress.
    override suspend fun download(progress: (Float?) -> Unit) {
        progress(null)
        lock.withLock { recognizer().download() }
        installed = true
    }

    override suspend fun load() = lock.withLock {
        if (warm) return@withLock
        check(isInstalled()) { "Download the speech model first." }
        // The SDK loads the model on its first transcription. Half a second of silence moves that wait from the
        // user's first take to the warm-up.
        recognizer().transcribe(FloatArray(SILENCE))
        warm = true
    }

    override fun open(): Take = WholeTake()

    private suspend fun transcribe(pcm: FloatArray): String = lock.withLock {
        if (pcm.isEmpty()) "" else recognizer().transcribe(pcm).text.trim()
    }

    @Synchronized
    override fun unload() {
        // Creating a recognizer is cheap; the next load reads the files again. close() waits for a running take.
        voz?.close()
        voz = null
        warm = false
    }

    @Synchronized
    private fun recognizer(): Voz = voz ?: run {
        dir.mkdirs()
        Voz(context, dir.absolutePath).also { voz = it }
    }

    /** Keeps the audio of a take, and reads all of it at the end. */
    private inner class WholeTake : Take {
        private var pcm = FloatArray(10 * 16_000)
        private var count = 0
        private var ended = false

        @Synchronized
        override fun push(chunk: ShortArray, offset: Int, length: Int): Boolean {
            if (ended) return false
            val take = minOf(length, Take.MAX_SAMPLES - count)
            if (count + take > pcm.size) pcm = pcm.copyOf(minOf(pcm.size * 2, Take.MAX_SAMPLES))
            for (i in 0 until take) pcm[count + i] = chunk[offset + i] / 32768f
            count += take
            return count < Take.MAX_SAMPLES
        }

        @Synchronized
        override fun stop(): Boolean {
            ended = true
            return false
        }

        override suspend fun finish(): String = transcribe(end())

        override fun cancel() {
            end()
        }

        @Synchronized
        private fun end(): FloatArray {
            ended = true
            return pcm.copyOf(count)
        }
    }

    private companion object {
        const val SILENCE = 8_000
    }
}

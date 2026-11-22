package io.github.hi9841.tdt

import android.system.Os
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.hi9841.tdt.engine.SpeechModel
import io.github.hi9841.tdt.engine.TritEngine
import io.github.hi9841.tdt.model.ModelPack
import io.github.hi9841.tdt.model.ModelStore
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Runs a Trit model on a 12.3 s speech clip, through takes as the app does. Arguments (`am instrument -e`):
 * `model` redux (default) or unified; `precision` auto, int8 or q8_0 in place of the app's choice; `kernels` v2
 * selects Trit's second kernel set; `isa` forces a Trit kernel path (scalar, neon); `threads` sets the worker
 * threads. The last three are for a comparison, and they work only when the engine was not in use in this process
 * before the test (read the `loaded` line of the TDT log). The model must be in the app (downloaded in the app), or
 * copied by adb into the app's external files folder (Android/data/io.github.hi9841.tdt/files/models/).
 */
@RunWith(AndroidJUnit4::class)
class EngineTest {
    @Test
    fun transcribesTheSpeechClipWordForWord() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val arguments = InstrumentationRegistry.getArguments()
        arguments.getString("isa")?.let { Os.setenv("TRIT_ISA", it, true) }
        arguments.getString("kernels")?.let { Os.setenv("TRIT_KERNELS", it, true) }
        arguments.getString("threads")?.let { Os.setenv("TRIT_THREADS", it, true) }
        val unified = arguments.getString("model") == "unified"
        val pack = (if (unified) ModelPack.Unified else ModelPack.Redux)
            .let { pack -> arguments.getString("precision")?.let { pack.copy(precision = it) } ?: pack }
        val context = instrumentation.targetContext
        val store = ModelStore(context, pack)
        // Created by the app, so files adb pushes into it stay readable for the app.
        val external = File(context.getExternalFilesDir(null), "models/${pack.dir}").apply { mkdirs() }
        adopt(external, store)
        val seen = { dir: File -> dir.listFiles()?.joinToString { "${it.name}=${it.length()}" } ?: "unreadable" }
        assumeTrue(
            "Download the model in the app first. App storage: ${seen(store.dir)}; files: ${seen(context.filesDir)}; " +
                "models: ${seen(File(context.filesDir, "models"))}; data: ${seen(context.dataDir)}; external: ${seen(external)} " +
                "(external root ${seen(context.getExternalFilesDir(null)!!)})",
            store.isInstalled(),
        )
        val pcm = readWav(instrumentation.context.assets.open("ten.wav").readBytes())
        val engine = TritEngine(store, if (unified) SpeechModel.Unified else SpeechModel.Redux)

        val loadStart = System.nanoTime()
        engine.load()
        val loadMs = (System.nanoTime() - loadStart) / 1_000_000
        take(engine, pcm, realtime = false) // Warm-up, as after a bubble tap that loaded the model on focus.
        // All the audio at once: the wait is the time of the model for the whole clip.
        val atOnce = List(3) { take(engine, pcm, realtime = false) }
        // At speaking pace, as from the microphone: the wait is the wait of the user after the stop.
        val spoken = take(engine, pcm, realtime = true)
        val resident = File("/proc/self/status").readLines().first { it.startsWith("VmRSS") }
        engine.unload()
        val peak = File("/proc/self/status").readLines().first { it.startsWith("VmHWM") }
        Log.i(
            "EngineTest",
            "${pack.dir} precision=${pack.precision} isa=${arguments.getString("isa") ?: "auto"} " +
                "kernels=${arguments.getString("kernels") ?: "v1"} threads=${arguments.getString("threads") ?: "auto"} " +
                "$peak $resident load=${loadMs}ms " +
                "atOnce=${atOnce.map { it.second }}ms spoken=${spoken.second}ms audio=${pcm.size / 16}ms " +
                "text=${spoken.first}",
        )
        // Unified ends this clip without the final period; every kernel path gives the same text.
        val expected = if (unified) EXPECTED.removeSuffix(".") else EXPECTED
        assertEquals(expected, atOnce.first().first)
        assertEquals(expected, spoken.first)
    }

    /**
     * Gives [pcm] to a take in the recorder's 20 ms chunks, then stops it. Returns the text and the wait in ms
     * from the stop to the text.
     */
    private suspend fun take(engine: TritEngine, pcm: ShortArray, realtime: Boolean): Pair<String, Long> {
        val take = engine.open()
        val start = System.nanoTime()
        var offset = 0
        while (offset < pcm.size) {
            val length = minOf(CHUNK, pcm.size - offset)
            if (realtime) {
                val due = start + (offset + length) * 1_000_000_000L / 16_000
                delay(maxOf(0, (due - System.nanoTime()) / 1_000_000))
            }
            take.push(pcm, offset, length)
            offset += length
        }
        val stop = System.nanoTime()
        take.stop()
        val text = take.finish()
        return text to (System.nanoTime() - stop) / 1_000_000
    }

    /** Moves model files that adb put in the external files folder into app storage, where the app looks. */
    private fun adopt(from: File, store: ModelStore) {
        if (!from.isDirectory) return
        store.dir.mkdirs()
        for (file in store.pack.files) {
            val source = File(from, file.name)
            val target = File(store.dir, file.name)
            if (source.length() == file.bytes && target.length() != file.bytes) {
                source.copyTo(target, overwrite = true)
                source.delete()
            }
        }
    }

    /** Reads the data chunk of a 16 kHz mono PCM16 WAV. Chunk sizes vary by writer, so walk the chunks. */
    private fun readWav(bytes: ByteArray): ShortArray {
        val buffer = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
        var at = 12
        while (at + 8 <= bytes.size) {
            val id = String(bytes, at, 4, Charsets.US_ASCII)
            val size = buffer.getInt(at + 4)
            if (id == "data") {
                val samples = ByteBuffer.wrap(bytes, at + 8, size).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
                return ShortArray(samples.remaining()) { samples.get(it) }
            }
            at += 8 + size + (size and 1)
        }
        error("No data chunk in the clip")
    }

    private companion object {
        /** 20 ms at 16 kHz, as the recorder gives. */
        const val CHUNK = 320
        const val EXPECTED = "Talk, don't type. This is a short test of the TDT speech engine running on Android. " +
            "The quick brown fox jumps over the lazy dog, and then it goes home to rest."
    }
}

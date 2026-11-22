package io.github.hi9841.tdt.engine

/**
 * The speech models the user can pick. Each one brings its own runtime and its own download. [minMemoryBytes] is
 * the least total memory (ActivityManager.MemoryInfo.totalMem) a phone needs to run the model.
 */
enum class SpeechModel(
    val title: String,
    val detail: String,
    val megabytes: Int,
    val credit: String,
    val minMemoryBytes: Long = 0,
) {
    Redux(
        title = "Parakeet Redux",
        detail = "Small and fast. English. Uses your dictionary.",
        megabytes = 179,
        credit = "Parakeet Redux by Moondream, CC-BY-4.0, on the Trit engine (PolyForm Noncommercial 1.0.0).",
    ),
    Unified(
        title = "Parakeet Unified English",
        detail = "Fewer mistakes, the model TDT desktop uses. Larger and a little slower. Uses your dictionary.",
        megabytes = 731,
        credit = "NVIDIA Parakeet Unified English 0.6B (NVIDIA Open Model License), Q8_0 GGUF by handy-computer, " +
            "on the Trit engine (PolyForm Noncommercial 1.0.0).",
        // About 0.8 GB resident with the model loaded; leave room for the app the user types in.
        minMemoryBytes = 3L shl 30,
    ),
    Voz(
        title = "Voz",
        detail = "25 languages. Its Android files are 1.2 GB and it needs much memory. Does not use your dictionary.",
        // The Android download: encoder.tflite (1,207.5 MB) and the decoder, embedding and mel files. The 337 MB
        // in the Voz docs is the Core ML build for Apple devices.
        megabytes = 1245,
        credit = "Voz by Desert Ant Labs, Desert Ant Labs Source-Available License.",
        // Measured 2026-10-29: 3.9 GB peak resident memory on a 6 GB emulator; killed for low memory on 4 GB.
        // A 6 GB phone reports about 5.5 GB of total memory.
        minMemoryBytes = 5L shl 30,
    ),
}

/** Turns takes of 16 kHz mono speech into text with one [SpeechModel]. */
interface SpeechEngine {
    val model: SpeechModel

    /**
     * True when the model files are on the phone. Can read the files, so call it off the main thread, or from
     * Main.immediate when an engine answers from memory after the first call.
     */
    suspend fun isInstalled(): Boolean

    /** Downloads the model files. [progress] gets 0 to 1, or null when the SDK does not tell. */
    suspend fun download(progress: (Float?) -> Unit)

    /**
     * True when the model stays in memory while the app runs, so no take waits for a load. False: the app frees
     * the model after some idle time, because its memory is too much to keep.
     */
    val staysLoaded: Boolean get() = false

    /** Loads the model into memory. Safe to call again while loaded. */
    suspend fun load()

    /** Terms to favor, one per entry, from the next take on. Engines without a dictionary ignore them. */
    fun setDictionary(terms: List<String>) {}

    /** Starts a take. It loads the model itself when the model is not in memory. */
    fun open(): Take

    /** Frees the model's memory. The next [load] reads the files again. */
    fun unload()
}

/**
 * One take: the audio from the start of the recording to its text. The recorder thread gives it the audio while
 * it comes ([push]); [finish] or [cancel] ends it, and after that it takes no more audio.
 */
interface Take {
    /**
     * [length] samples of 16 kHz mono speech from [offset] of [chunk]. Returns false when the take needs no more
     * audio: it is [MAX_SAMPLES] long, or the user asked it to stop and the last word has ended.
     */
    fun push(chunk: ShortArray, offset: Int, length: Int): Boolean

    /** Closes audio input. Returns false so capture can stop. */
    fun stop(): Boolean

    /** Ends the take and waits for its text. */
    suspend fun finish(): String

    /** Drops the take. */
    fun cancel()

    companion object {
        /** 120 s, the same cap as TDT desktop, so memory stays bounded. */
        const val MAX_SAMPLES = 120 * 16_000
    }
}

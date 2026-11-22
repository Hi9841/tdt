package io.github.hi9841.tdt

import android.os.SystemClock
import android.util.Log
import java.io.File
import io.github.hi9841.tdt.audio.Recorder
import io.github.hi9841.tdt.engine.SpeechEngine
import io.github.hi9841.tdt.engine.Take
import io.github.hi9841.tdt.text.Fillers
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** Where one dictation is. The bubble and the app screens draw from this. */
sealed interface DictationState {
    data object Ready : DictationState
    /** The voice level is in [Dictation.level], so a level change does not redraw everything that reads the state. */
    data object Recording : DictationState
    data object Processing : DictationState
    /** [waitMillis]: from the stop to the text, as the user waited for it. */
    data class Done(val text: String, val waitMillis: Long = 0) : DictationState
    data class Failed(val message: String) : DictationState
}

/**
 * Record, transcribe, clean up, remember. One instance for the whole app, so the app screens and the bubble
 * share one model in memory. [selected] gives the engine of the model the user picked. Call it on the main
 * thread ([scope] runs there).
 */
class Dictation(
    private val scope: CoroutineScope,
    private val selected: () -> SpeechEngine,
    private val history: History,
    private val settings: Settings,
) {
    private val _state = MutableStateFlow<DictationState>(DictationState.Ready)
    val state: StateFlow<DictationState> = _state.asStateFlow()

    private val _level = MutableStateFlow(0f)
    /** The voice level (RMS) of the last 20 ms while recording. */
    val level: StateFlow<Float> = _level.asStateFlow()

    /** The live wave of the open microphone, as on TDT desktop. Read it on each frame. */
    val wave get() = recorder.envelope

    private val _lastTake = MutableStateFlow<String?>(null)
    /** The times of the last take, also from the bubble, so the user can read them out when a take was slow. */
    val lastTake: StateFlow<String?> = _lastTake.asStateFlow()

    private var unloadJob: Job? = null
    /** The engine whose model is in memory, or loads now. */
    private var loaded: SpeechEngine? = null
    /** The take that records now. */
    private var take: Take? = null
    // The recorder calls back on its own thread; the work happens on the main thread.
    private val recorder = Recorder(
        onLevel = { level -> _level.value = level },
        onEnd = { scope.launch { finish() } },
        onFail = { message -> scope.launch { fail(message) } },
    )

    /** Loads the model ahead of a tap: when the bubble service starts, and when a text field gets focus. */
    fun warmUp() {
        scope.launch {
            runCatching { use(selected()).load() }
            if (take == null) scheduleUnload()
        }
    }

    fun start() {
        if (take != null) return
        unloadJob?.cancel()
        val engine = use(selected())
        engine.setDictionary(settings.dictionary)
        // The model reads the take while the user talks, so the text is ready soon after the stop.
        val take = engine.open().also { take = it }
        recorder.start(take)
        _level.value = 0f
        _state.value = DictationState.Recording
        // Loading overlaps with speech, so a cold model costs less wait after the user stops.
        warmUp()
    }

    /**
     * Stops recording and returns the cleaned text, or null when there is nothing to insert. [onClosed] runs when
     * the take has all its audio: a stop cue played before that is in the take, and the take then waits for a new
     * read of its last part.
     */
    suspend fun finish(onClosed: () -> Unit = {}): String? {
        val take = take ?: return null
        this.take = null
        val asked = SystemClock.elapsedRealtime()
        _state.value = DictationState.Processing
        val samples = recorder.stop()
        val closed = SystemClock.elapsedRealtime()
        onClosed()
        return try {
            val raw = take.finish()
            val text = if (settings.removeFillers) Fillers.remove(raw) else raw
            val done = SystemClock.elapsedRealtime()
            // Times only, never the words.
            Log.i(TAG, "take audio=${samples / 16}ms wait=${done - asked}ms (last word ${closed - asked}ms, model ${done - closed}ms) chars=${text.length}")
            _lastTake.value = "Last take: %.1f s, wait %d ms (microphone %d, model %d), cores %s".format(
                samples / 16_000.0, done - asked, closed - asked, done - closed, allowedCores(),
            )
            if (text.isNotBlank()) history.add(text)
            _state.value = DictationState.Done(text, done - asked)
            text.ifBlank { null }
        } catch (error: Exception) {
            _state.value = DictationState.Failed(error.message ?: "Transcription failed.")
            null
        } finally {
            scheduleUnload()
        }
    }

    /** Stops recording and drops the audio: the user tapped cancel. */
    fun cancel() {
        if (drop()) _state.value = DictationState.Ready
    }

    fun reset() {
        if (take == null) _state.value = DictationState.Ready
    }

    /** Frees the model now, for example after the user picks another one. */
    fun release() {
        unloadJob?.cancel()
        unload()
    }

    /** The microphone did not open. */
    private fun fail(message: String) {
        if (drop()) _state.value = DictationState.Failed(message)
    }

    /** Ends the take that records, with no text. False when none records. */
    private fun drop(): Boolean {
        val take = take ?: return false
        this.take = null
        recorder.cancel()
        take.cancel()
        scheduleUnload()
        return true
    }

    /** Makes [engine] the one in memory: the model of another engine leaves first. */
    private fun use(engine: SpeechEngine): SpeechEngine {
        if (loaded !== engine) loaded?.unload()
        loaded = engine
        return engine
    }

    private fun unload() {
        loaded?.unload()
        loaded = null
    }

    /** Frees the model after some idle time, when its engine does not keep it. */
    private fun scheduleUnload() {
        unloadJob?.cancel()
        if (loaded?.staysLoaded == true) return
        unloadJob = scope.launch {
            delay(IDLE_UNLOAD_MILLIS)
            unload()
        }
    }

    /** The cores Android lets the app use now (its cpuset), for example "0-6". */
    private fun allowedCores(): String = runCatching {
        File("/proc/self/status").useLines { lines ->
            lines.first { it.startsWith("Cpus_allowed_list:") }.substringAfter(':').trim()
        }
    }.getOrDefault("?")

    companion object {
        private const val TAG = "TDT"
        const val IDLE_UNLOAD_MILLIS = 5 * 60 * 1000L
    }
}

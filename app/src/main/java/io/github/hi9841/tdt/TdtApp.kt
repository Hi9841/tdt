package io.github.hi9841.tdt

import android.app.Application
import io.github.hi9841.tdt.engine.SpeechEngine
import io.github.hi9841.tdt.engine.SpeechModel
import io.github.hi9841.tdt.engine.TritEngine
import io.github.hi9841.tdt.engine.VozEngine
import io.github.hi9841.tdt.model.ModelPack
import io.github.hi9841.tdt.model.ModelStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob

/** Holds the engines, settings and dictation pipeline that the app screens and the bubble share. */
class TdtApp : Application() {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val history by lazy { History(this) }
    val settings by lazy { Settings(this) }
    private val redux by lazy { TritEngine(ModelStore(this, ModelPack.Redux), SpeechModel.Redux) }
    private val unified by lazy { TritEngine(ModelStore(this, ModelPack.Unified), SpeechModel.Unified) }
    private val voz by lazy { VozEngine(this) }
    val dictation by lazy { Dictation(scope, { engine }, history, settings) }

    fun engineFor(model: SpeechModel): SpeechEngine = when (model) {
        SpeechModel.Redux -> redux
        SpeechModel.Unified -> unified
        SpeechModel.Voz -> voz
    }

    /** The engine of the model the user picked. */
    val engine: SpeechEngine get() = engineFor(settings.model)
}

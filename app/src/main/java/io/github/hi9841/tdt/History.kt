package io.github.hi9841.tdt

import android.content.Context
import io.github.hi9841.tdt.engine.SpeechModel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray

/** The last [MAX] transcripts, newest first, kept only on this phone. Same limit as TDT desktop. */
class History(context: Context) {
    private val prefs = context.getSharedPreferences("history", Context.MODE_PRIVATE)
    private val _items = MutableStateFlow(read())
    val items: StateFlow<List<String>> = _items.asStateFlow()

    fun add(text: String) = save((listOf(text) + _items.value).take(MAX))

    fun clear() = save(emptyList())

    private fun read(): List<String> {
        val array = JSONArray(prefs.getString(KEY, "[]"))
        return List(array.length()) { array.getString(it) }
    }

    private fun save(items: List<String>) {
        _items.value = items
        prefs.edit().putString(KEY, JSONArray(items).toString()).apply()
    }

    private companion object {
        const val MAX = 10
        const val KEY = "items"
    }
}

/** User settings. */
class Settings(context: Context) {
    private val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)

    var removeFillers: Boolean
        get() = prefs.getBoolean("remove_fillers", true)
        set(value) = prefs.edit().putBoolean("remove_fillers", value).apply()

    /** The desktop cues on start, stop, success and error. Silent anyway on silent or vibrate. */
    var sounds: Boolean
        get() = prefs.getBoolean("sounds", true)
        set(value) = prefs.edit().putBoolean("sounds", value).apply()

    var model: SpeechModel
        get() = SpeechModel.entries.firstOrNull { it.name == prefs.getString("model", null) } ?: SpeechModel.Redux
        set(value) = prefs.edit().putString("model", value.name).apply()

    /** Names and terms the user added, one per entry. TDT ships none. */
    var dictionary: List<String>
        get() = prefs.getString("dictionary", "").orEmpty().lines().map { it.trim() }.filter { it.isNotEmpty() }
        set(value) = prefs.edit().putString("dictionary", value.joinToString("\n")).apply()
}

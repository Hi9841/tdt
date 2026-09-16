package com.voicestt.mobile.model

sealed interface VoiceSttState {
    data object Idle : VoiceSttState
    data class Recording(val audioLevel: Float = 0f, val durationSecs: Long = 0L) : VoiceSttState
    data object Transcribing : VoiceSttState
    data class Success(val text: String, val copied: Boolean = true) : VoiceSttState
    data class Error(val message: String) : VoiceSttState
}

data class TranscriptionItem(
    val id: String,
    val text: String,
    val timestamp: Long,
    val durationMs: Long
)

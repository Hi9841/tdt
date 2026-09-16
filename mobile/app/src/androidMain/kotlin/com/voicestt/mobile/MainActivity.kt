package com.voicestt.mobile

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.*
import androidx.core.content.ContextCompat
import com.voicestt.mobile.engine.AudioRecordManager
import com.voicestt.mobile.engine.SherpaSenseVoiceEngine
import com.voicestt.mobile.model.TranscriptionItem
import com.voicestt.mobile.model.VoiceSttState
import com.voicestt.mobile.service.OverlayService
import com.voicestt.mobile.ui.VoiceSttScreen
import kotlinx.coroutines.launch
import java.util.UUID

class MainActivity : ComponentActivity() {

    private val audioRecorder = AudioRecordManager()
    private lateinit var sttEngine: SherpaSenseVoiceEngine

    private var hasMicPermission by mutableStateOf(false)
    private var isOverlayServiceRunning by mutableStateOf(false)
    private var uiState by mutableStateOf<VoiceSttState>(VoiceSttState.Idle)
    private val historyItems = mutableStateListOf<TranscriptionItem>()

    private val requestPermissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) { permissions ->
        hasMicPermission = permissions[Manifest.permission.RECORD_AUDIO] == true
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        checkPermissions()

        sttEngine = SherpaSenseVoiceEngine(this)

        setContent {
            val scope = rememberCoroutineScope()

            // Sync audio level into state when recording
            LaunchedEffect(audioRecorder.isRecording) {
                if (audioRecorder.isRecording) {
                    audioRecorder.audioLevel.collect { level ->
                        uiState = VoiceSttState.Recording(audioLevel = level)
                    }
                }
            }

            VoiceSttScreen(
                state = uiState,
                history = historyItems,
                isOverlayActive = isOverlayServiceRunning,
                onPushToTalkDown = {
                    if (!hasMicPermission) {
                        checkPermissions()
                        return@VoiceSttScreen
                    }
                    val started = audioRecorder.startRecording()
                    if (started) {
                        scope.launch { sttEngine.initialize() }
                        uiState = VoiceSttState.Recording(0f, 0L)
                    }
                },
                onPushToTalkUp = {
                    if (audioRecorder.isRecording) {
                        val samples = audioRecorder.stopRecording()
                        uiState = VoiceSttState.Transcribing
                        scope.launch {
                            val result = sttEngine.transcribe(samples)
                            result.onSuccess { text ->
                                if (text.isNotBlank()) {
                                    copyTextToClipboard(text)
                                    historyItems.add(
                                        0,
                                        TranscriptionItem(
                                            id = UUID.randomUUID().toString(),
                                            text = text,
                                            timestamp = System.currentTimeMillis(),
                                            durationMs = 0L
                                        )
                                    )
                                    uiState = VoiceSttState.Success(text, copied = true)
                                } else {
                                    uiState = VoiceSttState.Idle
                                    Toast.makeText(this@MainActivity, "No speech detected", Toast.LENGTH_SHORT).show()
                                }
                            }.onFailure { err ->
                                uiState = VoiceSttState.Error(err.message ?: "Transcription error")
                            }
                        }
                    }
                },
                onToggleOverlay = {
                    toggleFloatingOverlay()
                },
                onCopyItem = { text ->
                    copyTextToClipboard(text)
                    Toast.makeText(this, "Copied to clipboard", Toast.LENGTH_SHORT).show()
                }
            )
        }
    }

    private fun checkPermissions() {
        val mic = ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        hasMicPermission = mic
        val needed = mutableListOf<String>()
        if (!mic) needed.add(Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val notif = ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
            if (!notif) needed.add(Manifest.permission.POST_NOTIFICATIONS)
        }
        if (needed.isNotEmpty()) {
            requestPermissionLauncher.launch(needed.toTypedArray())
        }
    }

    private fun toggleFloatingOverlay() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M && !Settings.canDrawOverlays(this)) {
            val intent = Intent(
                Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                Uri.parse("package:$packageName")
            )
            startActivity(intent)
            Toast.makeText(this, "Please grant overlay permission for the floating mic bubble", Toast.LENGTH_LONG).show()
            return
        }

        val serviceIntent = Intent(this, OverlayService::class.java)
        if (!isOverlayServiceRunning) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                startForegroundService(serviceIntent)
            } else {
                startService(serviceIntent)
            }
            isOverlayServiceRunning = true
            Toast.makeText(this, "Floating mic bubble enabled", Toast.LENGTH_SHORT).show()
        } else {
            stopService(serviceIntent)
            isOverlayServiceRunning = false
            Toast.makeText(this, "Floating mic bubble stopped", Toast.LENGTH_SHORT).show()
        }
    }

    private fun copyTextToClipboard(text: String) {
        val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        val clip = ClipData.newPlainText("Voice STT", text)
        clipboard.setPrimaryClip(clip)
    }

    override fun onDestroy() {
        super.onDestroy()
        audioRecorder.release()
        sttEngine.release()
    }
}

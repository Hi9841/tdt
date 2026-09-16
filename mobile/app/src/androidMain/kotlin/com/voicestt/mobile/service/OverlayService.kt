package com.voicestt.mobile.service

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.IBinder
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.Toast
import androidx.core.app.NotificationCompat
import com.voicestt.mobile.engine.AudioRecordManager
import com.voicestt.mobile.engine.SherpaSenseVoiceEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlin.math.abs

class OverlayService : Service() {

    private lateinit var windowManager: WindowManager
    private var floatView: FrameLayout? = null
    private var micIcon: ImageView? = null

    private val audioRecorder = AudioRecordManager()
    private lateinit var sttEngine: SherpaSenseVoiceEngine
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    private var initialX = 0
    private var initialY = 0
    private var initialTouchX = 0f
    private var initialTouchY = 0f
    private var isClick = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        sttEngine = SherpaSenseVoiceEngine(this)

        startForegroundService()
        initFloatingBubble()
    }

    private fun startForegroundService() {
        val channelId = "voice_stt_overlay"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                channelId,
                "Voice STT Floating Overlay",
                NotificationManager.IMPORTANCE_LOW
            )
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(channel)
        }

        val notification: Notification = NotificationCompat.Builder(this, channelId)
            .setContentTitle("Voice STT Active")
            .setContentText("Tap or hold the floating bubble to dictate")
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()

        startForeground(1001, notification)
    }

    @SuppressLint("ClickableViewAccessibility")
    private fun initFloatingBubble() {
        windowManager = getSystemService(Context.WINDOW_SERVICE) as WindowManager

        val layoutType = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
        } else {
            @Suppress("DEPRECATION")
            WindowManager.LayoutParams.TYPE_PHONE
        }

        val density = resources.displayMetrics.density
        val sizePx = (56 * density).toInt()

        val params = WindowManager.LayoutParams(
            sizePx,
            sizePx,
            layoutType,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                    WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = 40
            y = 300
        }

        floatView = FrameLayout(this).apply {
            contentDescription = "Voice dictation bubble"
            isClickable = true
            val bg = GradientDrawable().apply {
                shape = GradientDrawable.OVAL
                setColor(Color.parseColor("#18181b")) // Dark slate
                setStroke((2 * density).toInt(), Color.parseColor("#27272a"))
            }
            background = bg
            elevation = 16f
        }

        micIcon = ImageView(this).apply {
            setImageResource(android.R.drawable.ic_btn_speak_now)
            setColorFilter(Color.parseColor("#10b981")) // Teal
            val padding = (14 * density).toInt()
            setPadding(padding, padding, padding, padding)
        }
        floatView?.addView(micIcon)

        floatView?.setOnTouchListener { _, event ->
            when (event.action) {
                MotionEvent.ACTION_DOWN -> {
                    initialX = params.x
                    initialY = params.y
                    initialTouchX = event.rawX
                    initialTouchY = event.rawY
                    isClick = true
                    true
                }
                MotionEvent.ACTION_MOVE -> {
                    val dx = (event.rawX - initialTouchX).toInt()
                    val dy = (event.rawY - initialTouchY).toInt()
                    if (abs(dx) > 10 || abs(dy) > 10) {
                        isClick = false
                    }
                    params.x = initialX + dx
                    params.y = initialY + dy
                    windowManager.updateViewLayout(floatView, params)
                    true
                }
                MotionEvent.ACTION_UP -> {
                    if (isClick) {
                        onBubbleClicked()
                    }
                    true
                }
                else -> false
            }
        }

        try {
            windowManager.addView(floatView, params)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    private fun onBubbleClicked() {
        if (!audioRecorder.isRecording) {
            startDictation()
        } else {
            stopDictationAndTranscribe()
        }
    }

    private fun startDictation() {
        val started = audioRecorder.startRecording()
        if (started) {
            setBubbleRecordingState(true)
            // Prewarm the recognizer while the user is speaking. The model is
            // released after each transcription to keep idle memory low.
            scope.launch { sttEngine.initialize() }
        } else {
            Toast.makeText(this, "Microphone permission required", Toast.LENGTH_SHORT).show()
        }
    }

    private fun stopDictationAndTranscribe() {
        val samples = audioRecorder.stopRecording()
        setBubbleRecordingState(false)
        setBubbleTranscribingState(true)

        scope.launch {
            val result = sttEngine.transcribe(samples)
            setBubbleTranscribingState(false)
            result.onSuccess { text ->
                if (text.isNotBlank()) {
                    copyToClipboard(text)
                    Toast.makeText(this@OverlayService, "Copied: $text", Toast.LENGTH_LONG).show()
                } else {
                    Toast.makeText(this@OverlayService, "No speech detected", Toast.LENGTH_SHORT).show()
                }
            }.onFailure { err ->
                Toast.makeText(this@OverlayService, "STT error: ${err.message}", Toast.LENGTH_SHORT).show()
            }
        }
    }

    private fun copyToClipboard(text: String) {
        val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        val clip = ClipData.newPlainText("Voice STT", text)
        clipboard.setPrimaryClip(clip)
    }

    private fun setBubbleRecordingState(recording: Boolean) {
        val bg = floatView?.background as? GradientDrawable ?: return
        if (recording) {
            bg.setColor(Color.parseColor("#450a0a")) // Dark red
            bg.setStroke(4, Color.parseColor("#ef4444")) // Crimson border
            micIcon?.setColorFilter(Color.parseColor("#ef4444"))
        } else {
            bg.setColor(Color.parseColor("#18181b"))
            bg.setStroke(2, Color.parseColor("#27272a"))
            micIcon?.setColorFilter(Color.parseColor("#10b981"))
        }
    }

    private fun setBubbleTranscribingState(transcribing: Boolean) {
        val bg = floatView?.background as? GradientDrawable ?: return
        if (transcribing) {
            bg.setColor(Color.parseColor("#1e1b4b")) // Indigo
            bg.setStroke(4, Color.parseColor("#6366f1"))
            micIcon?.setColorFilter(Color.parseColor("#818cf8"))
        } else {
            bg.setColor(Color.parseColor("#18181b"))
            bg.setStroke(2, Color.parseColor("#27272a"))
            micIcon?.setColorFilter(Color.parseColor("#10b981"))
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        audioRecorder.release()
        sttEngine.release()
        scope.cancel()
        floatView?.let {
            try {
                windowManager.removeView(it)
            } catch (_: Exception) {}
        }
    }
}

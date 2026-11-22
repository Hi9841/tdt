package io.github.hi9841.tdt.bubble

import android.Manifest
import android.accessibilityservice.AccessibilityService
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.PixelFormat
import android.graphics.Rect
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.PowerManager
import android.view.Display
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.WindowInsets
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import android.widget.FrameLayout
import android.widget.Toast
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.setViewTreeLifecycleOwner
import androidx.savedstate.setViewTreeSavedStateRegistryOwner
import io.github.hi9841.tdt.DictationState
import io.github.hi9841.tdt.MainActivity
import io.github.hi9841.tdt.TdtApp
import io.github.hi9841.tdt.audio.Sound
import io.github.hi9841.tdt.audio.Sounds
import kotlin.math.roundToInt
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

/**
 * The bubble: a side tab attached to the left or right screen edge that shows when an editable text field has
 * focus. Tap it, talk, then tap ✓ (or tap ✕ to drop the take), or hold it while you talk. Drag it along an edge
 * or to the other edge; push it into its edge to tuck it into a thin handle; drag it to the bottom of the screen
 * to hide it for 10 minutes. It never shows on password fields.
 */
class BubbleService : AccessibilityService(), BubbleGestures {
    private val app get() = application as TdtApp
    private lateinit var windowManager: WindowManager
    private lateinit var inserter: TextInserter
    private lateinit var position: BubblePosition
    private lateinit var sounds: Sounds
    private val lifecycle = OverlayLifecycle()
    private var host: BubbleTouch? = null
    private val params = WindowManager.LayoutParams(
        WindowManager.LayoutParams.WRAP_CONTENT,
        WindowManager.LayoutParams.WRAP_CONTENT,
        WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
        // Not focusable: the keyboard must keep the input focus while the bubble is on screen. Hardware
        // accelerated: a window that a service adds is drawn in software without this flag.
        WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS or
            WindowManager.LayoutParams.FLAG_HARDWARE_ACCELERATED,
        PixelFormat.TRANSLUCENT,
    )
    /** The layout the window has now, so a call that changes nothing does not cost a window relayout. */
    private var applied: WindowManager.LayoutParams? = null
    private var face: ComposeView? = null
    private var collapseJob: Job? = null
    private var fastestRate = 0f

    // What the face draws besides the dictation state. Compose reads these, so a change redraws the bubble.
    private val notice = mutableStateOf<Notice?>(null)
    private val onRight = mutableStateOf(true)
    private val tucked = mutableStateOf(false)
    /** False while the bubble slides back into its edge, just before its window goes. */
    private val visible = mutableStateOf(false)
    private val pressed = mutableStateOf(false)
    private var hideJob: Job? = null

    /** The text field the current dictation writes into. */
    private var target: AccessibilityNodeInfo? = null
    /** True while a dictation that this bubble started is running. */
    private var session = false
    private var shown = false
    private var noticeJob: Job? = null
    private var startJob: Job? = null
    private var keyboardTop: Int? = null
    /** While the finger drags the bubble, focus events must not put it back in place. */
    private var dragging = false
    /** Keeps the CPU on during a take if the screen turns off anyway (power button, cover). */
    private val wakeLock by lazy {
        getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "tdt:dictation")
            .apply { setReferenceCounted(false) }
    }

    override fun onServiceConnected() {
        windowManager = getSystemService(WindowManager::class.java)
        inserter = TextInserter(this)
        position = BubblePosition(this)
        sounds = Sounds(this).also { it.prewarm() }
        // One height in every state, so the window never changes height.
        params.height = px(BubbleHeight)
        // Not `display`: a service is not a visual context, and asking it for its display throws.
        fastestRate = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
            ?.supportedModes?.maxOfOrNull { it.refreshRate } ?: 0f
        onRight.value = position.onRight
        tucked.value = position.tucked
        lifecycle.resume()
        app.scope.launch { app.dictation.state.collectLatest(::onDictationState) }
        // The service starts with the phone and after an update, long before a take. A model that stays in
        // memory loads now, so no take waits for it.
        if (app.engine.staysLoaded) app.dictation.warmUp()
        registerReceiver(showReceiver, IntentFilter(HiddenNotice.ACTION_SHOW), RECEIVER_NOT_EXPORTED)
    }

    /** "Show bubble" in the hidden-bubble notification. */
    private val showReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            position.show()
            HiddenNotice.cancel(context)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                performGlobalAction(GLOBAL_ACTION_DISMISS_NOTIFICATION_SHADE)
            }
            refresh()
        }
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent) {
        if (event.packageName == packageName) return
        refresh()
    }

    override fun onInterrupt() = Unit

    override fun onDestroy() {
        keepAwake(false)
        sounds.release()
        unregisterReceiver(showReceiver)
        hide(force = true)
        lifecycle.destroy()
        super.onDestroy()
    }

    /** Shows the bubble on an editable, non-password field that has input focus; hides it otherwise. */
    private fun refresh() {
        keyboardTop = windows.firstOrNull { it.type == AccessibilityWindowInfo.TYPE_INPUT_METHOD }
            ?.let { window -> Rect().also(window::getBoundsInScreen).top }
        val focused = findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
        val editable = focused != null && focused.isEditable && !focused.isPassword && focused.packageName != packageName
        when {
            session || notice.value != null -> place() // Keep the bubble through a take and its notice.
            editable && !position.isHidden() -> {
                if (focused != target) target = focused
                show()
            }
            else -> hide()
        }
    }

    private fun show() {
        hideJob?.cancel()
        visible.value = true
        if (!shown) {
            val view = host ?: createView().also { host = it }
            place()
            windowManager.addView(view, params)
            applied = WindowManager.LayoutParams().apply { copyFrom(params) }
            shown = true
            // A model that was not on the phone when the service started loads here, before the tap.
            app.dictation.warmUp()
        } else {
            place()
        }
    }

    /** Slides the bubble into its edge, then removes its window. [force]: remove it now. */
    private fun hide(force: Boolean = false) {
        if (!shown || (session && !force)) return
        if (force) {
            hideJob?.cancel()
            remove()
            return
        }
        if (hideJob?.isActive == true) return
        visible.value = false
        hideJob = app.scope.launch {
            delay(EXIT_MILLIS)
            remove()
        }
    }

    private fun remove() {
        host?.let(windowManager::removeView)
        applied = null
        shown = false
    }

    /**
     * Attaches the bubble to its edge at the remembered height, lifted above the keyboard when it would cover
     * it. The gravity is the edge itself, so the bubble stays flush while it grows and shrinks.
     */
    private fun place() {
        if (dragging) return
        val metrics = windowManager.currentWindowMetrics.bounds
        val height = px(BubbleHeight)
        val gap = px(12.dp)
        params.gravity = Gravity.TOP or if (position.onRight) Gravity.END else Gravity.START
        params.x = 0
        alignFace()
        val wanted = (position.heightFraction * metrics.height()).roundToInt()
        // The overlay is laid out below the status bar, but window bounds are in screen coordinates.
        val windowTop = windowManager.currentWindowMetrics.windowInsets
            .getInsets(WindowInsets.Type.statusBars() or WindowInsets.Type.displayCutout()).top
        params.y = keyboardTop?.let { minOf(wanted, it - windowTop - height - gap) } ?: wanted
        commit()
    }

    /** Sends the layout to the window manager only when it changed: every call is a window relayout. */
    private fun commit() {
        val view = host ?: return
        if (!shown) return
        val last = applied
        if (last != null && last.x == params.x && last.y == params.y && last.width == params.width &&
            last.gravity == params.gravity && last.flags == params.flags &&
            last.preferredRefreshRate == params.preferredRefreshRate
        ) {
            return
        }
        windowManager.updateViewLayout(view, params)
        applied = WindowManager.LayoutParams().apply { copyFrom(params) }
    }

    /**
     * Fixes the window's width while the pill changes size, so the pill animates inside the window and the
     * window itself never resizes mid-animation (each resize is a round trip to the system and drops frames).
     * [width] null: shrink the window back around the face once the motion has settled. While fixed, the
     * bubble asks for the display's fastest refresh rate.
     */
    private fun holdWidth(width: Dp?, touchable: Boolean = true) {
        collapseJob?.cancel()
        val touch = WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE
        params.flags = if (touchable) params.flags and touch.inv() else params.flags or touch
        if (width != null) {
            params.width = px(width)
            params.preferredRefreshRate = fastestRate
            commit()
            return
        }
        commit()
        collapseJob = app.scope.launch {
            delay(SETTLE_MILLIS)
            params.width = WindowManager.LayoutParams.WRAP_CONTENT
            params.preferredRefreshRate = 0f
            commit()
        }
    }

    /** The face hugs the bubble's edge inside a window that can be wider than it. */
    private fun alignFace() {
        val view = face ?: return
        val gravity = Gravity.CENTER_VERTICAL or if (position.onRight) Gravity.END else Gravity.START
        val layout = view.layoutParams as FrameLayout.LayoutParams
        if (layout.gravity != gravity) {
            layout.gravity = gravity
            view.layoutParams = layout
        }
    }

    private fun px(value: Dp) = (value.value * resources.displayMetrics.density).roundToInt()

    private fun createView(): BubbleTouch = BubbleTouch(this, this).apply {
        contentDescription = "TDT dictation"
        // The tab sits in the edge strip where back gestures start; ask the system to leave touches on it alone.
        addOnLayoutChangeListener { view, _, _, _, _, _, _, _, _ ->
            view.systemGestureExclusionRects = listOf(Rect(0, 0, view.width, view.height))
        }
        setViewTreeLifecycleOwner(lifecycle)
        setViewTreeSavedStateRegistryOwner(lifecycle)
        val view = ComposeView(this@BubbleService).apply {
            setViewTreeLifecycleOwner(lifecycle)
            setViewTreeSavedStateRegistryOwner(lifecycle)
            setContent {
                val state by app.dictation.state.collectAsState()
                BubbleFace(state, app.dictation.wave, notice.value, onRight.value, tucked.value, visible.value, pressed.value)
            }
        }
        face = view
        addView(
            view,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.CENTER_VERTICAL or if (position.onRight) Gravity.END else Gravity.START,
            ),
        )
    }

    // Gestures

    override fun onPressed(down: Boolean) {
        pressed.value = down
    }

    override fun onTap(x: Float) {
        if (tucked.value) {
            setTucked(false)
            return
        }
        when (app.dictation.state.value) {
            is DictationState.Recording -> if (inCancel(x)) cancel() else stop()
            DictationState.Processing -> Unit
            else -> start()
        }
    }

    /** The ✕ is the inner end of the listening pill: its left end on the right edge, and the other way round. */
    private fun inCancel(x: Float): Boolean {
        val width = host?.width ?: return false
        val cancel = CancelWidth.value * resources.displayMetrics.density
        return if (position.onRight) x < cancel else x > width - cancel
    }

    override fun onHoldStart() {
        if (tucked.value) return
        if (app.dictation.state.value !is DictationState.Recording) start()
    }

    // A short hold is a slow tap: keep recording until ✓, so the take is not a fraction of a second.
    override fun onHoldEnd(heldMillis: Long) {
        if (heldMillis >= SLOW_TAP_MILLIS) stop()
    }

    override fun onDrag(dx: Float, dy: Float) {
        val view = host ?: return
        if (!dragging) {
            // Move in screen coordinates from the left while the finger has the bubble.
            dragging = true
            val width = windowManager.currentWindowMetrics.bounds.width()
            if (position.onRight) params.x = width - view.width - params.x
            params.gravity = Gravity.TOP or Gravity.START
        }
        params.x += dx.roundToInt()
        params.y += dy.roundToInt()
        commit()
    }

    override fun onDragEnd() {
        dragging = false
        val view = host ?: return
        val metrics = windowManager.currentWindowMetrics.bounds
        if (params.y > metrics.height() * 0.88f && !session) {
            position.hideForTenMinutes()
            HiddenNotice.post(this, BubblePosition.HIDE_MILLIS)
            hide()
            place()
            Toast.makeText(this, "TDT bubble hidden for 10 minutes", Toast.LENGTH_SHORT).show()
            return
        }
        val right = params.x + view.width / 2 > metrics.width() / 2
        // Pushed into its own edge by more than a third of its width: tuck it into a handle.
        val pushed = if (right) params.x + view.width - metrics.width() else -params.x
        position.onRight = right
        onRight.value = right
        position.heightFraction = params.y.toFloat() / metrics.height()
        if (!session) setTucked(pushed > view.width / 3 || (tucked.value && pushed > 0))
        place()
    }

    private fun setTucked(value: Boolean) {
        // Tab and handle change size; keep the window at the tab's width until the change has settled.
        if (value != tucked.value) {
            holdWidth(TabWidth)
            holdWidth(null)
        }
        tucked.value = value
        position.tucked = value
        host?.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
    }

    // Dictation

    private fun start() {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            // The permission prompt needs an activity; the app screen asks for it.
            startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return
        }
        // Main.immediate: once the engine knows its files, this runs at once, so hold-to-talk starts with the hold.
        startJob = app.scope.launch {
            if (!app.engine.isInstalled()) {
                startActivity(Intent(this@BubbleService, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                return@launch
            }
            noticeJob?.cancel()
            notice.value = null
            session = true
            holdWidth(ListenWidth)
            keepAwake(true)
            cue(Sound.Start)
            app.dictation.start()
        }
    }

    private fun stop() {
        // A release before the first check of the files ends: no take.
        startJob?.cancel()
        if (app.dictation.state.value !is DictationState.Recording) return
        // Undispatched: the take learns of the stop before the vibration reaches the microphone. The sound waits
        // until the microphone has closed, which is at once when the user was quiet.
        app.scope.launch(start = CoroutineStart.UNDISPATCHED) {
            app.dictation.finish(onClosed = { sound(Sound.Stop) })
        }
        haptic(Sound.Stop)
    }

    private fun cancel() {
        cue(Sound.Stop)
        session = false
        keepAwake(false)
        app.dictation.cancel()
        holdWidth(null)
        refresh()
    }

    /** A haptic tick always; the desktop sound too when the user keeps sounds on. */
    private fun cue(sound: Sound) {
        haptic(sound)
        sound(sound)
    }

    private fun haptic(sound: Sound) {
        host?.performHapticFeedback(
            if (sound == Sound.Error) HapticFeedbackConstants.REJECT else HapticFeedbackConstants.CONFIRM,
        )
    }

    private fun sound(sound: Sound) {
        if (app.settings.sounds) sounds.play(sound)
    }

    private fun onDictationState(state: DictationState) {
        if (!session) return
        when (state) {
            is DictationState.Done -> {
                session = false
                keepAwake(false)
                if (state.text.isBlank()) {
                    cue(Sound.Error)
                    showNotice(Notice("No speech heard"), NOTICE_MILLIS)
                } else {
                    val result = inserter.insert(target, state.text)
                    cue(Sound.Success)
                    val label = if (result == InsertResult.COPIED) "Copied, paste it" else "Inserted"
                    showNotice(Notice(label, "${state.waitMillis} ms"), NOTICE_MILLIS)
                }
            }
            is DictationState.Failed -> {
                session = false
                keepAwake(false)
                cue(Sound.Error)
                showNotice(Notice(state.message.substringBefore(". ").trimEnd('.'), error = true), ERROR_MILLIS)
            }
            else -> Unit
        }
    }

    /** Shows a notice in the pill, then shrinks back to the tab (or hides, if the text field went away). */
    private fun showNotice(value: Notice, millis: Long) {
        noticeJob?.cancel()
        // Wide enough for the longest notice. Not touchable: the notice only reports, and the clear part of the
        // window must not block a Send button under it.
        holdWidth(NoticeWidth, touchable = false)
        notice.value = value
        noticeJob = app.scope.launch {
            delay(millis)
            notice.value = null
            app.dictation.reset()
            holdWidth(null)
            refresh()
        }
    }

    /** The screen stays on from the start of a take until its text is in, as during a call. */
    private fun keepAwake(on: Boolean) {
        val keep = WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON
        val flags = if (on) params.flags or keep else params.flags and keep.inv()
        params.flags = flags
        commit()
        if (on) wakeLock.acquire(AWAKE_LIMIT_MILLIS) else if (wakeLock.isHeld) wakeLock.release()
    }

    private companion object {
        const val NOTICE_MILLIS = 1_400L
        /** Longer than the pop spring takes to settle. */
        const val SETTLE_MILLIS = 450L
        /** The face's slide back into the edge (180 ms) and a frame to spare. */
        const val EXIT_MILLIS = 200L
        val NoticeWidth = 300.dp
        const val ERROR_MILLIS = 2_400L
        /** Holds shorter than this (after the 300 ms hold delay) count as a tap. */
        const val SLOW_TAP_MILLIS = 600L
        /** The 120 s recording cap plus time to write the text. */
        const val AWAKE_LIMIT_MILLIS = 150_000L
    }
}

/** Remembered bubble edge, height and tuck, and the 10-minute hide. */
class BubblePosition(context: Context) {
    private val prefs = context.getSharedPreferences("bubble", Context.MODE_PRIVATE)

    var onRight: Boolean
        get() = prefs.getBoolean("on_right", true)
        set(value) = prefs.edit().putBoolean("on_right", value).apply()

    var heightFraction: Float
        get() = prefs.getFloat("height", 0.55f)
        set(value) = prefs.edit().putFloat("height", value.coerceIn(0.05f, 0.85f)).apply()

    /** Tucked into its edge as a thin handle; a tap brings it back. */
    var tucked: Boolean
        get() = prefs.getBoolean("tucked", false)
        set(value) = prefs.edit().putBoolean("tucked", value).apply()

    fun isHidden() = System.currentTimeMillis() < prefs.getLong("hidden_until", 0)

    fun hideForTenMinutes() = prefs.edit().putLong("hidden_until", System.currentTimeMillis() + HIDE_MILLIS).apply()

    fun show() = prefs.edit().remove("hidden_until").apply()

    companion object {
        const val HIDE_MILLIS = 10 * 60_000L
    }
}

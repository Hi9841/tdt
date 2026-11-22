package io.github.hi9841.tdt.bubble

import android.annotation.SuppressLint
import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.MotionEvent
import android.view.ViewConfiguration
import android.widget.FrameLayout

/** What the user did to the bubble. */
interface BubbleGestures {
    /** The finger went down on the bubble (true) or left it, lifted or started a drag (false). */
    fun onPressed(down: Boolean)

    /** [x]: where the tap landed in the bubble, from its left side, so the ✕ can cancel. */
    fun onTap(x: Float)
    fun onHoldStart()
    /** [heldMillis]: how long the hold lasted after [onHoldStart]. */
    fun onHoldEnd(heldMillis: Long)
    fun onDrag(dx: Float, dy: Float)
    fun onDragEnd()
}

/**
 * Turns touches on the bubble window into tap, hold and drag. Uses screen coordinates, because the window
 * itself moves during a drag and view-local coordinates would move with it.
 */
@SuppressLint("ViewConstructor")
class BubbleTouch(context: Context, private val gestures: BubbleGestures) : FrameLayout(context) {
    private val slop = ViewConfiguration.get(context).scaledTouchSlop
    private val handler = Handler(Looper.getMainLooper())
    private var downX = 0f
    private var downY = 0f
    private var lastX = 0f
    private var lastY = 0f
    private var mode = Mode.NONE
    private var holdStartedAt = 0L
    private var tapX = 0f

    private enum class Mode { NONE, PRESSED, HOLDING, DRAGGING }

    private val hold = Runnable {
        if (mode == Mode.PRESSED) {
            mode = Mode.HOLDING
            holdStartedAt = SystemClock.uptimeMillis()
            gestures.onHoldStart()
        }
    }

    override fun onInterceptTouchEvent(event: MotionEvent) = true

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = event.rawX; downY = event.rawY
                tapX = event.x
                lastX = downX; lastY = downY
                mode = Mode.PRESSED
                gestures.onPressed(true)
                handler.postDelayed(hold, HOLD_MILLIS)
            }
            MotionEvent.ACTION_MOVE -> {
                if (mode == Mode.PRESSED && Math.hypot((event.rawX - downX).toDouble(), (event.rawY - downY).toDouble()) > slop) {
                    handler.removeCallbacks(hold)
                    mode = Mode.DRAGGING
                    gestures.onPressed(false)
                }
                if (mode == Mode.DRAGGING) gestures.onDrag(event.rawX - lastX, event.rawY - lastY)
                lastX = event.rawX; lastY = event.rawY
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                handler.removeCallbacks(hold)
                gestures.onPressed(false)
                // The system took the touch, for example for a back gesture from the screen edge.
                if (event.actionMasked == MotionEvent.ACTION_CANCEL) Log.i("TDT", "bubble touch cancelled in mode $mode")
                when (mode) {
                    Mode.PRESSED -> if (event.actionMasked == MotionEvent.ACTION_UP) performClick()
                    Mode.HOLDING -> gestures.onHoldEnd(SystemClock.uptimeMillis() - holdStartedAt)
                    Mode.DRAGGING -> gestures.onDragEnd()
                    Mode.NONE -> Unit
                }
                mode = Mode.NONE
            }
        }
        return true
    }

    // Accessibility services and switch access click through here too.
    override fun performClick(): Boolean {
        super.performClick()
        // A click from an accessibility tool has no touch point: treat it as a tap on the middle.
        gestures.onTap(if (mode == Mode.PRESSED) tapX else width / 2f)
        return true
    }

    private companion object {
        /** Press longer than this without moving and the bubble records until release. */
        const val HOLD_MILLIS = 300L
    }
}

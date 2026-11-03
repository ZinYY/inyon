package tech.abhashchakraborty.inyon.device

import android.animation.Animator
import android.animation.AnimatorListenerAdapter
import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.RectF
import android.os.Build
import android.provider.Settings
import android.util.AttributeSet
import android.view.View
import android.view.animation.AccelerateDecelerateInterpolator
import android.view.animation.DecelerateInterpolator

/**
 * The scanner's viewfinder: a dimmed scrim with a rounded square cut out, and corner brackets that
 * breathe while it looks and snap onto the code once it has one. Still when animations are off.
 */
class ScannerOverlay(context: Context, attrs: AttributeSet?) : View(context, attrs) {

    /** The cut-out, in view coordinates; set on layout. */
    val hole = RectF()
    var onHoleChanged: ((RectF) -> Unit)? = null

    private val density = resources.displayMetrics.density
    private val radius = 22 * density
    private val bracket = 30 * density
    private val scrim = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = 0xB814161B.toInt() }
    private val corners = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFFF1F0EC.toInt()
        style = Paint.Style.STROKE
        strokeWidth = 4 * density
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }
    private val scrimPath = Path().apply { fillType = Path.FillType.EVEN_ODD }
    private val cornerPath = Path()

    /** Where the brackets sit now: the hole while looking, the code after a hit. */
    private val frame = RectF()
    private var breath = 0f
    private var breathing: ValueAnimator? = null
    private var snapped = false

    private val animate: Boolean
        get() =
            if (Build.VERSION.SDK_INT >= 26) {
                ValueAnimator.areAnimatorsEnabled()
            } else {
                Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) != 0f
            }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        val side = minOf(w, h) * 0.68f
        val cx = w / 2f
        // a little above the middle, leaving room for the buttons below
        val cy = h * 0.42f
        hole.set(cx - side / 2, cy - side / 2, cx + side / 2, cy + side / 2)
        if (!snapped) frame.set(hole)
        scrimPath.reset()
        scrimPath.addRect(0f, 0f, w.toFloat(), h.toFloat(), Path.Direction.CW)
        scrimPath.addRoundRect(hole, radius, radius, Path.Direction.CW)
        onHoleChanged?.invoke(hole)
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        if (!animate || snapped) return
        breathing = ValueAnimator.ofFloat(0f, 1f).apply {
            duration = 1400
            repeatMode = ValueAnimator.REVERSE
            repeatCount = ValueAnimator.INFINITE
            interpolator = AccelerateDecelerateInterpolator()
            addUpdateListener {
                breath = it.animatedValue as Float
                invalidate()
            }
            start()
        }
    }

    override fun onDetachedFromWindow() {
        breathing?.cancel()
        breathing = null
        super.onDetachedFromWindow()
    }

    /** Moves the brackets onto the code (view coordinates), then runs [then]. */
    fun snapTo(code: Rect, then: () -> Unit) {
        snapped = true
        breathing?.cancel()
        breath = 0f
        val pad = 10 * density
        val from = RectF(frame)
        val to = RectF(code.left - pad, code.top - pad, code.right + pad, code.bottom + pad)
        if (!animate) {
            frame.set(to)
            invalidate()
            postDelayed({ then() }, 250)
            return
        }
        ValueAnimator.ofFloat(0f, 1f).apply {
            duration = 180
            interpolator = DecelerateInterpolator()
            addUpdateListener {
                val t = it.animatedValue as Float
                frame.set(
                    from.left + (to.left - from.left) * t,
                    from.top + (to.top - from.top) * t,
                    from.right + (to.right - from.right) * t,
                    from.bottom + (to.bottom - from.bottom) * t,
                )
                invalidate()
            }
            addListener(
                object : AnimatorListenerAdapter() {
                    override fun onAnimationEnd(animation: Animator) {
                        postDelayed({ then() }, 220)
                    }
                },
            )
            start()
        }
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawPath(scrimPath, scrim)
        // breathing pulls the brackets in by up to 5dp and back
        val inset = breath * 5 * density
        val l = frame.left + inset
        val t = frame.top + inset
        val r = frame.right - inset
        val b = frame.bottom - inset
        val rad = minOf(radius, (r - l) / 4)
        val len = minOf(bracket, (r - l) / 3)
        cornerPath.reset()
        corner(l, t, rad, len, 1f, 1f)
        corner(r, t, rad, len, -1f, 1f)
        corner(l, b, rad, len, 1f, -1f)
        corner(r, b, rad, len, -1f, -1f)
        canvas.drawPath(cornerPath, corners)
    }

    /** One L-shaped bracket with a rounded elbow at (x, y), its arms pointing along (dx, dy). */
    private fun corner(x: Float, y: Float, rad: Float, len: Float, dx: Float, dy: Float) {
        cornerPath.moveTo(x, y + dy * len)
        cornerPath.lineTo(x, y + dy * rad)
        cornerPath.quadTo(x, y, x + dx * rad, y)
        cornerPath.lineTo(x + dx * len, y)
    }
}

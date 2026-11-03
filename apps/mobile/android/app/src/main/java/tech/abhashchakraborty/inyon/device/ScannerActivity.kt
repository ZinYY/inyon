package tech.abhashchakraborty.inyon.device

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.Rect
import android.os.Bundle
import android.os.SystemClock
import android.util.Log
import android.util.Size
import android.view.View
import android.widget.ImageButton
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.OptIn
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ExperimentalGetImage
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.core.TorchState
import androidx.camera.core.resolutionselector.AspectRatioStrategy
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.android.gms.common.moduleinstall.ModuleInstall
import com.google.android.gms.common.moduleinstall.ModuleInstallRequest
import com.google.mlkit.common.MlKitException
import com.google.mlkit.vision.barcode.BarcodeScanner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import tech.abhashchakraborty.inyon.R
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * Scans the pairing code tether-node shows. It captures on its own as soon as an Inyon code is in
 * view, and the camera runs only while this screen is in front. Results go back to scanQr().
 *
 * Plain CameraX use cases rather than a CameraController: the controller brings in the video
 * library, which is most of CameraX's size.
 */
class ScannerActivity : ComponentActivity() {

    private lateinit var preview: PreviewView
    private lateinit var overlay: ScannerOverlay
    private lateinit var hint: TextView
    private lateinit var torch: ImageButton

    private lateinit var scanner: BarcodeScanner
    private var provider: ProcessCameraProvider? = null
    private var camera: Camera? = null
    private var analysis: ExecutorService? = null
    private val haptics by lazy { Haptics(this) }
    private var hapticsOn = true

    private var running = false
    @Volatile private var done = false
    private var idle = false
    private var installing = false
    private var lastReject = 0L

    private val showIdle = Runnable {
        idle = true
        if (!installing) hint.text = IDLE
    }
    private val restoreHint = Runnable { hint.text = if (installing) INSTALLING else if (idle) IDLE else POINT }

    private val askCamera =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            // a grant is picked up by onResume, which follows the permission dialog
            if (!granted) finishWith(Intent().putExtra(EXTRA_DENIED, true))
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge(SystemBarStyle.dark(Color.TRANSPARENT), SystemBarStyle.dark(Color.TRANSPARENT))
        super.onCreate(savedInstanceState)
        Orientation.apply(this)
        setContentView(R.layout.activity_scanner)
        hapticsOn = intent.getBooleanExtra(EXTRA_HAPTICS, true)

        preview = findViewById(R.id.scanner_preview)
        overlay = findViewById(R.id.scanner_overlay)
        hint = findViewById(R.id.scanner_hint)
        torch = findViewById(R.id.scanner_torch)
        hint.accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
        val gap = 28 * resources.displayMetrics.density
        overlay.onHoleChanged = { hole -> hint.translationY = hole.bottom + gap }

        val bottom = findViewById<View>(R.id.scanner_bottom)
        val padding = bottom.paddingBottom
        ViewCompat.setOnApplyWindowInsetsListener(bottom) { v, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            v.setPadding(v.paddingLeft, v.paddingTop, v.paddingRight, padding + bars.bottom)
            insets
        }

        findViewById<View>(R.id.scanner_close).setOnClickListener { finishWith(null) }
        findViewById<View>(R.id.scanner_type).setOnClickListener { finishWith(Intent().putExtra(EXTRA_TYPE, true)) }
        torch.setOnClickListener {
            val c = camera ?: return@setOnClickListener
            c.cameraControl.enableTorch(c.cameraInfo.torchState.value != TorchState.ON)
        }

        scanner = BarcodeScanning.getClient(BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build())
        if (!hasCamera() && savedInstanceState == null) askCamera.launch(Manifest.permission.CAMERA)
    }

    override fun onResume() {
        super.onResume()
        if (hasCamera()) start()
    }

    override fun onPause() {
        stop()
        super.onPause()
    }

    override fun onDestroy() {
        scanner.close()
        super.onDestroy()
    }

    private fun hasCamera() = ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

    private fun start() {
        if (running || done) return
        running = true
        if (!idle) hint.postDelayed(showIdle, IDLE_AFTER_MS)
        val future = ProcessCameraProvider.getInstance(this)
        future.addListener(
            {
                val p = try {
                    future.get()
                } catch (e: Exception) {
                    Log.w(TAG, "no camera provider: $e")
                    return@addListener
                }
                provider = p
                // paused again before the provider was ready
                if (running) bind(p)
            },
            ContextCompat.getMainExecutor(this),
        )
    }

    private fun bind(p: ProcessCameraProvider) {
        // both 4:3, so the frames see what the preview shows and boxes map straight onto it
        val ratio = AspectRatioStrategy.RATIO_4_3_FALLBACK_AUTO_STRATEGY
        val shown = Preview.Builder()
            .setResolutionSelector(ResolutionSelector.Builder().setAspectRatioStrategy(ratio).build())
            .build()
        shown.setSurfaceProvider(preview.surfaceProvider)
        val frames = ImageAnalysis.Builder()
            .setResolutionSelector(
                ResolutionSelector.Builder()
                    .setAspectRatioStrategy(ratio)
                    .setResolutionStrategy(ResolutionStrategy(Size(1280, 960), ResolutionStrategy.FALLBACK_RULE_CLOSEST_LOWER_THEN_HIGHER))
                    .build(),
            )
            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
            .build()
        val executor = Executors.newSingleThreadExecutor().also { analysis = it }
        frames.setAnalyzer(executor, ::analyze)

        p.unbindAll()
        val c = try {
            p.bindToLifecycle(this, CameraSelector.DEFAULT_BACK_CAMERA, shown, frames)
        } catch (e: Exception) {
            Log.w(TAG, "could not start the camera: $e")
            return
        }
        camera = c
        torch.visibility = if (c.cameraInfo.hasFlashUnit()) View.VISIBLE else View.INVISIBLE
        c.cameraInfo.torchState.observe(this) { state -> showTorch(state == TorchState.ON) }
    }

    /** The camera goes off whenever the screen leaves the front, not just when it closes. */
    private fun stop() {
        if (!running) return
        running = false
        hint.removeCallbacks(showIdle)
        camera?.cameraInfo?.torchState?.removeObservers(this)
        camera = null
        provider?.unbindAll()
        analysis?.shutdown()
        analysis = null
    }

    /** On the analysis thread; the frame stays open until the decoder is done with it. */
    @OptIn(ExperimentalGetImage::class)
    private fun analyze(frame: ImageProxy) {
        val image = frame.image
        if (image == null || done) return frame.close()
        val rotation = frame.imageInfo.rotationDegrees
        // the decoder reports boxes in the upright image
        val upright = if (rotation % 180 == 0) Size(frame.width, frame.height) else Size(frame.height, frame.width)
        scanner.process(InputImage.fromMediaImage(image, rotation))
            .addOnCompleteListener(ContextCompat.getMainExecutor(this)) { task ->
                frame.close()
                if (task.isSuccessful) onCodes(task.result.orEmpty(), upright) else onFailure(task.exception)
            }
    }

    private fun onCodes(codes: List<Barcode>, image: Size) {
        if (done || !running) return
        if (installing) {
            installing = false
            restoreHint.run()
        }
        if (codes.isEmpty()) return
        val hit = codes.firstOrNull { isPairLink(textOf(it)) } ?: return reject()

        done = true
        hint.removeCallbacks(showIdle)
        hint.removeCallbacks(restoreHint)
        if (hapticsOn) haptics.play("success", overlay)
        val text = textOf(hit)
        val box = hit.boundingBox?.let { toView(it, image) } ?: Rect().also { overlay.hole.round(it) }
        overlay.snapTo(box) { finishWith(Intent().putExtra(EXTRA_TEXT, text)) }
    }

    /** From the upright frame to the preview, which fills the view and crops the overflow. */
    private fun toView(box: Rect, image: Size): Rect {
        val w = preview.width.toFloat()
        val h = preview.height.toFloat()
        val scale = maxOf(w / image.width, h / image.height)
        val dx = (w - image.width * scale) / 2
        val dy = (h - image.height * scale) / 2
        return Rect(
            (box.left * scale + dx).toInt(),
            (box.top * scale + dy).toInt(),
            (box.right * scale + dx).toInt(),
            (box.bottom * scale + dy).toInt(),
        )
    }

    private fun onFailure(e: Exception?) {
        if (done || !running) return
        if ((e as? MlKitException)?.errorCode != MlKitException.UNAVAILABLE) {
            Log.w(TAG, "decoding failed: $e")
            return
        }
        // the decoder comes from Google Play services and may still be downloading
        if (installing) return
        installing = true
        hint.text = INSTALLING
        ModuleInstall.getClient(this)
            .installModules(ModuleInstallRequest.newBuilder().addApi(scanner).build())
            .addOnFailureListener { err -> Log.w(TAG, "could not request the decoder: $err") }
    }

    private fun reject() {
        val now = SystemClock.elapsedRealtime()
        if (now - lastReject < REJECT_EVERY_MS) return
        lastReject = now
        if (hapticsOn) haptics.play("warning", overlay)
        hint.text = NOT_INYON
        hint.removeCallbacks(restoreHint)
        hint.postDelayed(restoreHint, REJECT_EVERY_MS)
    }

    private fun showTorch(on: Boolean) {
        torch.isActivated = on
        torch.contentDescription = if (on) "Turn torch off" else "Turn torch on"
        torch.backgroundTintList = if (on) ColorStateList.valueOf(PAPER) else null
        torch.imageTintList = ColorStateList.valueOf(if (on) INK else PAPER)
    }

    private fun finishWith(data: Intent?) {
        setResult(if (data == null) RESULT_CANCELED else RESULT_OK, data)
        finish()
    }

    companion object {
        const val EXTRA_TEXT = "text"
        const val EXTRA_TYPE = "type"
        const val EXTRA_DENIED = "denied"
        const val EXTRA_HAPTICS = "haptics"
        private const val TAG = "InyonScanner"

        private const val POINT = "Point at the code tether-node shows"
        private const val IDLE = "Hold steady, about 20 cm away. Or type the code."
        private const val NOT_INYON = "That's not an Inyon code"
        private const val INSTALLING = "Getting the scanner ready…"
        private const val IDLE_AFTER_MS = 12_000L
        private const val REJECT_EVERY_MS = 2_500L
        private val INK = 0xFF1D1C1A.toInt()
        private val PAPER = 0xFFF1F0EC.toInt()

        private fun textOf(code: Barcode) = code.rawValue ?: code.rawBytes?.toString(Charsets.UTF_8)

        fun intent(context: Context, haptics: Boolean) = Intent(context, ScannerActivity::class.java).putExtra(EXTRA_HAPTICS, haptics)
    }
}

// the same links the web app's paste field takes: a computer's code, or a hub's (inyon://hub?…,
// or the download page with it after the #). tether:// is how a laptop from before the rename prints its code
private val PAIR_LINK = Regex("""^((?:inyon|tether)://pair\?|inyon://hub\?|https://\S+[?&]m=|https://\S+#hub\?)""")

/** Whether a scanned code is an Inyon pairing link rather than some other QR code. */
internal fun isPairLink(text: String?) = text != null && PAIR_LINK.containsMatchIn(text.trim())

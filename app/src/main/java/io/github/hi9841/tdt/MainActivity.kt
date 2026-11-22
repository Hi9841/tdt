package io.github.hi9841.tdt

import android.Manifest
import android.accessibilityservice.AccessibilityServiceInfo
import android.app.ActivityManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.accessibility.AccessibilityManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.hi9841.tdt.bubble.BubbleService
import io.github.hi9841.tdt.bubble.HiddenNotice
import io.github.hi9841.tdt.engine.SpeechModel
import io.github.hi9841.tdt.engine.TritEngine
import io.github.hi9841.tdt.model.isUnmetered
import io.github.hi9841.tdt.ui.theme.Brand
import io.github.hi9841.tdt.ui.theme.Motion
import io.github.hi9841.tdt.ui.theme.TdtTheme
import io.github.hi9841.tdt.ui.theme.pressScale
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val app = application as TdtApp
        setContent { TdtTheme { HomeScreen(app) } }
    }
}

/** Where one model's files are. */
private sealed interface Files {
    /** The engine is checking its files; Voz takes seconds for this on the first check. */
    data object Checking : Files
    data object Missing : Files
    /** [fraction] is null when the engine does not report progress. */
    data class Downloading(val fraction: Float?) : Files
    data object Installed : Files
    data class Failed(val message: String) : Files
}

/**
 * One screen, top to bottom: who TDT is and whether it is ready; the setup steps until they are done; a place to
 * try dictation; the speech models; the settings; the history; the credits.
 */
@Composable
private fun HomeScreen(app: TdtApp) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var selected by remember { mutableStateOf(app.settings.model) }
    val files = remember { mutableStateMapOf<SpeechModel, Files>() }
    LaunchedEffect(Unit) {
        for (model in SpeechModel.entries) {
            files[model] = if (app.engineFor(model).isInstalled()) Files.Installed else Files.Missing
        }
    }
    var askMetered by remember { mutableStateOf<SpeechModel?>(null) }

    // Android changes these outside the app (Settings, permission dialogs), so read them again on every resume.
    var canRecord by remember { mutableStateOf(hasMicrophone(context)) }
    var bubbleOn by remember { mutableStateOf(isBubbleEnabled(context)) }
    LifecycleResumeEffect(Unit) {
        canRecord = hasMicrophone(context)
        bubbleOn = isBubbleEnabled(context)
        onPauseOrDispose { }
    }
    val micPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { canRecord = it }

    fun download(model: SpeechModel) {
        scope.launch {
            files[model] = Files.Downloading(0f)
            files[model] = try {
                app.engineFor(model).download { fraction -> files[model] = Files.Downloading(fraction) }
                Files.Installed
            } catch (error: Exception) {
                Files.Failed(error.message ?: "Download failed.")
            }
        }
    }

    fun ask(model: SpeechModel) = if (isUnmetered(context)) download(model) else askMetered = model

    fun select(model: SpeechModel) {
        if (model == selected) return
        selected = model
        app.settings.model = model
        app.dictation.release()
    }

    val installed = files[selected] == Files.Installed
    val bubbleDone = bubbleOn || !BuildConfig.BUBBLE
    val ready = installed && canRecord && bubbleDone
    val stepsLeft = listOf(installed, canRecord, bubbleDone).count { !it }

    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            Modifier
                .safeDrawingPadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(top = 24.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Hero(ready, stepsLeft, selected, Modifier.enter(0))
            if (!ready) {
                SetupCard(
                    selected = selected,
                    files = files[selected] ?: Files.Checking,
                    canRecord = canRecord,
                    bubbleOn = bubbleOn,
                    onDownload = { ask(selected) },
                    onMicrophone = { micPermission.launch(Manifest.permission.RECORD_AUDIO) },
                    modifier = Modifier.enter(1),
                )
            }
            if (installed) TryCard(app, canRecord, { micPermission.launch(Manifest.permission.RECORD_AUDIO) }, Modifier.enter(2))
            if (ready && BuildConfig.BUBBLE) TipsCard(Modifier.enter(3))
            Section("Speech model", Modifier.enter(4)) {
                val memory = remember { totalMemory(context) }
                SpeechModel.entries.forEach { model ->
                    ModelCard(
                        model = model,
                        files = files[model] ?: Files.Checking,
                        selected = model == selected,
                        fits = memory >= model.minMemoryBytes,
                        onSelect = { select(model) },
                        onDownload = { ask(model) },
                    )
                }
            }
            SettingsCard(app, selected, Modifier.enter(5))
            HistoryCard(app, Modifier.enter(6))
            About(app, selected, Modifier.enter(7))
        }
    }

    askMetered?.let { model ->
        AlertDialog(
            onDismissRequest = { askMetered = null },
            title = { Text("Use mobile data?") },
            text = { Text("You are not on Wi-Fi. ${model.title} is ${model.megabytes} MB.") },
            confirmButton = { TextButton(onClick = { askMetered = null; download(model) }) { Text("Download") } },
            dismissButton = { TextButton(onClick = { askMetered = null }) { Text("Wait for Wi-Fi") } },
        )
    }
}

/** First open: each section rises 12 dp and fades in, 50 ms after the one above it. */
@Composable
private fun Modifier.enter(index: Int): Modifier {
    val progress = remember { Animatable(0f) }
    LaunchedEffect(Unit) { progress.animateTo(1f, Motion.enter(delay = index * Motion.STAGGER_MILLIS)) }
    val rise = with(LocalDensity.current) { 12.dp.toPx() }
    return graphicsLayer {
        alpha = progress.value
        translationY = (1f - progress.value) * rise
    }
}

@Composable
private fun Hero(ready: Boolean, stepsLeft: Int, model: SpeechModel, modifier: Modifier) {
    Column(modifier.fillMaxWidth().padding(vertical = 8.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(contentAlignment = Alignment.Center) {
            // A soft glow behind the orb, in the orb's own blue.
            Box(
                Modifier
                    .size(132.dp)
                    .background(Brush.radialGradient(listOf(Brand.Orb.copy(alpha = 0.28f), Color.Transparent))),
            )
            Image(painterResource(R.drawable.orb), contentDescription = null, Modifier.size(72.dp))
        }
        Text("TDT", style = MaterialTheme.typography.displaySmall)
        Spacer(Modifier.height(4.dp))
        Text(
            "Talk, don't type. Speech stays on this phone.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(16.dp))
        StatusPill(if (ready) "Ready · ${model.title}" else "$stepsLeft ${if (stepsLeft == 1) "step" else "steps"} to go", ready)
    }
}

@Composable
private fun StatusPill(label: String, good: Boolean) {
    val dot by animateColorAsState(
        if (good) MaterialTheme.colorScheme.tertiary else MaterialTheme.colorScheme.onSurfaceVariant,
        Motion.fast(),
        label = "dot",
    )
    Row(
        Modifier
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, CircleShape)
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(Modifier.size(8.dp).clip(CircleShape).background(dot))
        Text(label, style = MaterialTheme.typography.labelLarge)
    }
}

/** A raised card that groups one part of the screen. */
@Composable
private fun Card(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val shape = RoundedCornerShape(24.dp)
    Surface(
        shape = shape,
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = modifier
            .fillMaxWidth()
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, shape),
    ) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp), content = content)
    }
}

/** A titled group with no card of its own. */
@Composable
private fun Section(title: String, modifier: Modifier, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            title.uppercase(),
            Modifier.padding(start = 4.dp, top = 8.dp, bottom = 4.dp),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        content()
    }
}

@Composable
private fun SetupCard(
    selected: SpeechModel,
    files: Files,
    canRecord: Boolean,
    bubbleOn: Boolean,
    onDownload: () -> Unit,
    onMicrophone: () -> Unit,
    modifier: Modifier,
) {
    val context = LocalContext.current
    val done = listOf(files == Files.Installed, canRecord, bubbleOn || !BuildConfig.BUBBLE)
    // Only the first open step offers its action; the ones after it wait, dimmed.
    val current = done.indexOfFirst { !it }
    Card(modifier) {
        Text("Set up TDT", style = MaterialTheme.typography.titleLarge)
        Step(1, done[0], current == 0, "Download the speech model", modelDetail(selected, files)) {
            when (files) {
                is Files.Downloading -> Progress(files.fraction, selected.megabytes)
                Files.Checking -> Unit
                else -> {
                    if (files is Files.Failed) {
                        Text(files.message, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                    }
                    PrimaryButton("Download · ${selected.megabytes} MB", onDownload)
                }
            }
        }
        Step(2, done[1], current == 1, "Allow the microphone", "So TDT can hear you. The audio never leaves this phone.") {
            PrimaryButton("Allow microphone", onMicrophone)
        }
        if (BuildConfig.BUBBLE) {
            Step(
                3, done[2], current == 2, "Turn on the bubble",
                "In Accessibility, open TDT bubble and turn it on. If Android says the setting is restricted, open " +
                    "App info, tap ⋮, choose Allow restricted settings, and try again.",
            ) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PrimaryButton("Open Accessibility") { context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) }
                    OutlinedButton(onClick = {
                        context.startActivity(
                            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)),
                        )
                    }) { Text("App info") }
                }
            }
        }
    }
}

private fun modelDetail(model: SpeechModel, files: Files) = when (files) {
    Files.Installed -> "${model.title} is on this phone."
    Files.Checking -> "Checking ${model.title}…"
    else -> "${model.title}, ${model.megabytes} MB, once. After that TDT works offline."
}

/** One setup step: a number, or a check when done; its text; and its action while it is the current step. */
@Composable
private fun Step(
    number: Int,
    done: Boolean,
    current: Boolean,
    title: String,
    detail: String,
    action: @Composable ColumnScope.() -> Unit,
) {
    val waiting = !done && !current
    val fade by animateFloatAsState(if (waiting) 0.45f else 1f, Motion.fast(), label = "step")
    Row(Modifier.alpha(fade), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        val badge by animateColorAsState(
            when {
                done -> MaterialTheme.colorScheme.tertiary
                current -> MaterialTheme.colorScheme.primary
                else -> MaterialTheme.colorScheme.surfaceContainerHigh
            },
            Motion.fast(),
            label = "badge",
        )
        Box(Modifier.size(28.dp).clip(CircleShape).background(badge), contentAlignment = Alignment.Center) {
            Text(
                if (done) "✓" else "$number",
                color = if (done || current) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.labelLarge,
            )
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            AnimatedVisibility(
                current,
                enter = fadeIn(Motion.fast()) + expandVertically(Motion.fast()),
                exit = fadeOut(tween(120, easing = Motion.EaseOut)) + shrinkVertically(tween(160, easing = Motion.EaseOut)),
            ) {
                Column(Modifier.padding(top = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp), content = action)
            }
        }
    }
}

@Composable
private fun PrimaryButton(label: String, onClick: () -> Unit) {
    val source = remember { MutableInteractionSource() }
    Button(onClick = onClick, modifier = Modifier.pressScale(source), interactionSource = source) {
        Text(label, style = MaterialTheme.typography.labelLarge)
    }
}

@Composable
private fun Progress(fraction: Float?, megabytes: Int) {
    if (fraction == null) {
        LinearProgressIndicator(Modifier.fillMaxWidth().clip(CircleShape))
        return
    }
    val shown by animateFloatAsState(fraction, Motion.fast(), label = "progress")
    LinearProgressIndicator(progress = { shown }, modifier = Modifier.fillMaxWidth().clip(CircleShape))
    Text(
        "${(fraction * megabytes).toInt()} of $megabytes MB",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

/** Dictation inside the app: a big orb to tap, and what TDT heard under it. */
@Composable
private fun TryCard(app: TdtApp, canRecord: Boolean, onAskMicrophone: () -> Unit, modifier: Modifier) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val state by app.dictation.state.collectAsStateWithLifecycle()

    fun toggle() {
        when {
            state is DictationState.Recording -> scope.launch { app.dictation.finish() }
            state == DictationState.Processing -> Unit
            canRecord -> app.dictation.start()
            else -> onAskMicrophone()
        }
    }

    Card(modifier) {
        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
            TalkButton(app, state, onClick = ::toggle)
            Spacer(Modifier.height(12.dp))
            Text(
                when (state) {
                    DictationState.Ready, is DictationState.Done, is DictationState.Failed -> "Tap the orb and talk"
                    is DictationState.Recording -> "Listening · tap to stop"
                    DictationState.Processing -> "Writing…"
                },
                style = MaterialTheme.typography.titleMedium,
            )
        }
        AnimatedVisibility(
            state is DictationState.Done || state is DictationState.Failed,
            enter = fadeIn(Motion.fast()) + expandVertically(Motion.enter()),
            exit = fadeOut(tween(120, easing = Motion.EaseOut)) + shrinkVertically(tween(180, easing = Motion.EaseOut)),
        ) {
            when (val current = state) {
                is DictationState.Done -> Column(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(16.dp))
                        .background(MaterialTheme.colorScheme.surfaceContainerHigh)
                        .padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text(current.text.ifBlank { "No speech heard." }, style = MaterialTheme.typography.bodyLarge)
                    if (current.text.isNotBlank()) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                "${current.waitMillis} ms",
                                Modifier.weight(1f),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            CopyButton(current.text, context)
                        }
                    }
                }
                is DictationState.Failed -> Text(current.message, color = MaterialTheme.colorScheme.error)
                else -> Unit
            }
        }
    }
}

/** Copies, then says "Copied" for a moment, so the tap is confirmed. */
@Composable
private fun CopyButton(text: String, context: Context) {
    var copied by remember(text) { mutableStateOf(false) }
    LaunchedEffect(copied) {
        if (copied) {
            delay(1_200)
            copied = false
        }
    }
    TextButton(onClick = { copy(context, text); copied = true }) {
        Text(if (copied) "Copied" else "Copy", style = MaterialTheme.typography.labelLarge)
    }
}

/** The in-app talk button: the orb, with a ring that follows the voice while it listens. */
@Composable
private fun TalkButton(app: TdtApp, state: DictationState, onClick: () -> Unit) {
    val recording = state is DictationState.Recording
    val voice by app.dictation.level.collectAsStateWithLifecycle()
    val level = if (recording) (voice * 6f).coerceIn(0f, 1f) else 0f
    val ring by animateDpAsState(if (recording) (4 + 10 * level).dp else 0.dp, Motion.fast(), label = "ring")
    val ringColor by animateColorAsState(
        when (state) {
            is DictationState.Recording -> Brand.Foam.copy(alpha = 0.55f)
            DictationState.Processing -> Brand.Orb.copy(alpha = 0.35f)
            else -> Color.Transparent
        },
        Motion.fast(),
        label = "ringColor",
    )
    val source = remember { MutableInteractionSource() }
    Box(Modifier.size(120.dp).pressScale(source, pressed = 0.95f), contentAlignment = Alignment.Center) {
        Box(Modifier.size(96.dp + ring * 2).clip(CircleShape).background(ringColor))
        Image(
            painterResource(R.drawable.orb),
            contentDescription = null,
            Modifier
                .size(96.dp)
                .clip(CircleShape)
                .semantics {
                    role = Role.Button
                    contentDescription = if (recording) "Stop dictation" else "Start dictation"
                }
                .clickable(interactionSource = source, indication = null, onClick = onClick),
        )
    }
}

@Composable
private fun TipsCard(modifier: Modifier) {
    val context = LocalContext.current
    var canNotify by remember { mutableStateOf(HiddenNotice.allowed(context)) }
    val notifyPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { canNotify = it }
    Card(modifier) {
        Text("Using the bubble", style = MaterialTheme.typography.titleMedium)
        Tip("Tap any text field. The bubble shows on the edge of the screen.")
        Tip("Tap it and talk, then tap ✓. Tap ✕ to drop the take. Or hold it while you talk.")
        Tip("Drag it along the edge or to the other side. Push it into the edge to tuck it away.")
        Tip("Drag it to the bottom of the screen to hide it for 10 minutes.")
        if (!canNotify && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            TextButton(onClick = { notifyPermission.launch(Manifest.permission.POST_NOTIFICATIONS) }) {
                Text("Allow a notification that brings back a hidden bubble")
            }
        }
    }
}

@Composable
private fun Tip(text: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.Top) {
        Box(Modifier.padding(top = 8.dp).size(4.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
        Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** One speech model: pick it, and download it here. The picked card carries the accent border. */
@Composable
private fun ModelCard(
    model: SpeechModel,
    files: Files,
    selected: Boolean,
    fits: Boolean,
    onSelect: () -> Unit,
    onDownload: () -> Unit,
) {
    val source = remember { MutableInteractionSource() }
    val border by animateColorAsState(
        if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant,
        Motion.fast(),
        label = "border",
    )
    val shape = RoundedCornerShape(20.dp)
    Column(
        Modifier
            .fillMaxWidth()
            .pressScale(source, pressed = 0.98f)
            .clip(shape)
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .border(if (selected) 2.dp else 1.dp, border, shape)
            .selectable(selected, interactionSource = source, indication = null, enabled = fits, role = Role.RadioButton, onClick = onSelect)
            .alpha(if (fits) 1f else 0.45f)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(model.title, style = MaterialTheme.typography.titleMedium)
                Text(
                    if (fits) model.detail else "Needs a phone with more memory.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.size(12.dp))
            when (files) {
                Files.Installed -> Chip("On phone", MaterialTheme.colorScheme.tertiary)
                Files.Checking -> Chip("…", MaterialTheme.colorScheme.onSurfaceVariant)
                is Files.Downloading -> Chip("Downloading", MaterialTheme.colorScheme.primary)
                else -> if (fits) {
                    TextButton(onClick = onDownload) { Text("${model.megabytes} MB", style = MaterialTheme.typography.labelLarge) }
                }
            }
        }
        if (files is Files.Downloading) Progress(files.fraction, model.megabytes)
        if (files is Files.Failed) Text(files.message, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
    }
}

@Composable
private fun Chip(label: String, color: Color) {
    Text(
        label,
        Modifier
            .clip(CircleShape)
            .background(color.copy(alpha = 0.14f))
            .padding(horizontal = 10.dp, vertical = 4.dp),
        color = color,
        style = MaterialTheme.typography.labelMedium,
    )
}

@Composable
private fun SettingsCard(app: TdtApp, selected: SpeechModel, modifier: Modifier) {
    var removeFillers by remember { mutableStateOf(app.settings.removeFillers) }
    var sounds by remember { mutableStateOf(app.settings.sounds) }
    Section("Settings", modifier) {
        Card {
            SwitchRow("Remove filler words", "Drops um, uh and the like.", removeFillers) {
                removeFillers = it
                app.settings.removeFillers = it
            }
            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            SwitchRow("Sounds", "The TDT desktop cues on start, stop and done. Silent when the phone is.", sounds) {
                sounds = it
                app.settings.sounds = it
            }
            if (selected != SpeechModel.Voz) {
                HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                DictionaryEditor(app)
            }
        }
    }
}

@Composable
private fun SwitchRow(title: String, detail: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

/** The user's own names and terms, folded away until the user opens it. Saved as typed; the next take uses them. */
@Composable
private fun DictionaryEditor(app: TdtApp) {
    var text by remember { mutableStateOf(app.settings.dictionary.joinToString("\n")) }
    var open by remember { mutableStateOf(false) }
    val terms = text.lines().count { it.isNotBlank() }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(Modifier.fillMaxWidth().clickable { open = !open }, verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Dictionary", style = MaterialTheme.typography.bodyLarge)
                Text(
                    if (terms == 0) "Names and terms TDT should spell your way." else "$terms ${if (terms == 1) "term" else "terms"}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Text(if (open) "Done" else "Edit", color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelLarge)
        }
        AnimatedVisibility(
            open,
            enter = fadeIn(Motion.fast()) + expandVertically(Motion.enter()),
            exit = fadeOut(tween(120, easing = Motion.EaseOut)) + shrinkVertically(tween(180, easing = Motion.EaseOut)),
        ) {
            OutlinedTextField(
                value = text,
                onValueChange = {
                    text = it
                    app.settings.dictionary = it.lines()
                },
                modifier = Modifier.fillMaxWidth(),
                placeholder = { Text("One name or term per line, spelled as you want it written") },
                minLines = 3,
                shape = RoundedCornerShape(16.dp),
            )
        }
    }
}

@Composable
private fun HistoryCard(app: TdtApp, modifier: Modifier) {
    val context = LocalContext.current
    val history by app.history.items.collectAsStateWithLifecycle()
    if (history.isEmpty()) return
    var copied by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(copied) {
        if (copied != null) {
            delay(1_200)
            copied = null
        }
    }
    Section("History", modifier) {
        Card {
            // At most 10 takes (History.MAX). Tap one to copy it.
            history.forEachIndexed { index, take ->
                if (index > 0) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                Row(
                    Modifier.fillMaxWidth().clickable { copy(context, take); copied = take },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(take, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, maxLines = 3)
                    AnimatedVisibility(copied == take, enter = fadeIn(Motion.fast()), exit = fadeOut(Motion.fast())) {
                        Chip("Copied", MaterialTheme.colorScheme.tertiary)
                    }
                }
            }
            TextButton(onClick = { app.history.clear() }, Modifier.align(Alignment.End)) { Text("Clear history") }
        }
    }
}

/** Credits that the model licenses ask for, and the versions, so the user can see what runs. */
@Composable
private fun About(app: TdtApp, model: SpeechModel, modifier: Modifier) {
    val engine = remember { runCatching { "Trit ${TritEngine.version}" }.getOrDefault("Trit") }
    val lastTake by app.dictation.lastTake.collectAsStateWithLifecycle()
    Text(
        "TDT ${BuildConfig.VERSION_NAME} · $engine engine\n${lastTake?.let { "$it\n" }.orEmpty()}${model.credit}",
        modifier.fillMaxWidth().padding(top = 8.dp, start = 16.dp, end = 16.dp),
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
    )
}

private fun copy(context: Context, text: String) {
    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("TDT", text))
}

private fun totalMemory(context: Context): Long =
    ActivityManager.MemoryInfo().also { context.getSystemService(ActivityManager::class.java).getMemoryInfo(it) }.totalMem

private fun hasMicrophone(context: Context): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

private fun isBubbleEnabled(context: Context): Boolean {
    val manager = context.getSystemService(AccessibilityManager::class.java)
    return manager.getEnabledAccessibilityServiceList(AccessibilityServiceInfo.FEEDBACK_ALL_MASK)
        .any { it.resolveInfo.serviceInfo.let { info -> info.packageName == context.packageName && info.name == BubbleService::class.java.name } }
}

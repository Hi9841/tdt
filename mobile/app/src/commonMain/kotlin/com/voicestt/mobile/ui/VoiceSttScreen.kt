package com.voicestt.mobile.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.voicestt.mobile.model.TranscriptionItem
import com.voicestt.mobile.model.VoiceSttState

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun VoiceSttScreen(
    state: VoiceSttState,
    history: List<TranscriptionItem>,
    isOverlayActive: Boolean,
    onPushToTalkDown: () -> Unit,
    onPushToTalkUp: () -> Unit,
    onToggleOverlay: () -> Unit,
    onCopyItem: (String) -> Unit
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(
                            text = "Voice STT",
                            fontWeight = FontWeight.Bold,
                            color = Color(0xFFF4F4F5),
                            fontSize = 20.sp
                        )
                        Text(
                            text = "Sherpa-ONNX SenseVoice (On-Device)",
                            color = Color(0xFFA1A1AA),
                            fontSize = 12.sp
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = Color(0xFF09090B)
                )
            )
        },
        containerColor = Color(0xFF09090B)
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding)
                .padding(horizontal = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally
        ) {
            Spacer(modifier = Modifier.height(16.dp))

            // 1. Hero Dictation Card
            Card(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(24.dp),
                colors = CardDefaults.cardColors(containerColor = Color(0xFF18181B)),
                elevation = CardDefaults.cardElevation(defaultElevation = 2.dp)
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(24.dp),
                    horizontalAlignment = Alignment.CenterHorizontally
                ) {
                    val isRec = state is VoiceSttState.Recording
                    val isTranscribing = state is VoiceSttState.Transcribing

                    val buttonBg by animateColorAsState(
                        targetValue = when {
                            isRec -> Color(0xFFEF4444)
                            isTranscribing -> Color(0xFF6366F1)
                            else -> Color(0xFF10B981)
                        },
                        animationSpec = tween(200)
                    )

                    val buttonSize by animateDpAsState(
                        targetValue = if (isRec) 92.dp else 84.dp,
                        animationSpec = tween(150)
                    )

                    // Big tactile mic button
                    Box(
                        modifier = Modifier
                            .size(buttonSize)
                            .clip(CircleShape)
                            .background(buttonBg)
                            .clickable {
                                if (isRec) onPushToTalkUp() else onPushToTalkDown()
                            },
                        contentAlignment = Alignment.Center
                    ) {
                        Text(
                            text = when {
                                isRec -> "STOP"
                                isTranscribing -> "..."
                                else -> "REC"
                            },
                            fontWeight = FontWeight.Bold,
                            color = Color.White,
                            fontSize = 18.sp
                        )
                    }

                    Spacer(modifier = Modifier.height(16.dp))

                    // Status text and audio waveform
                    val statusText = when (state) {
                        is VoiceSttState.Idle -> "Tap or hold button to dictate"
                        is VoiceSttState.Recording -> "Listening..."
                        is VoiceSttState.Transcribing -> "Transcribing speech on-device..."
                        is VoiceSttState.Success -> "Transcribed & copied to clipboard"
                        is VoiceSttState.Error -> "Error: ${state.message}"
                    }

                    Text(
                        text = statusText,
                        fontWeight = FontWeight.Medium,
                        color = Color(0xFFE4E4E7),
                        fontSize = 14.sp
                    )

                    // Live waveform bars when recording
                    if (isRec) {
                        val level = (state as VoiceSttState.Recording).audioLevel
                        Spacer(modifier = Modifier.height(12.dp))
                        Row(
                            horizontalArrangement = Arrangement.spacedBy(4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.height(32.dp)
                        ) {
                            val h1 = (6 + level * 26).dp
                            val h2 = (10 + level * 32).dp
                            val h3 = (14 + level * 36).dp
                            val h4 = (8 + level * 28).dp
                            val h5 = (6 + level * 24).dp

                            Box(Modifier.width(4.dp).height(h1).clip(RoundedCornerShape(2.dp)).background(Color(0xFF10B981)))
                            Box(Modifier.width(4.dp).height(h2).clip(RoundedCornerShape(2.dp)).background(Color(0xFF34D399)))
                            Box(Modifier.width(4.dp).height(h3).clip(RoundedCornerShape(2.dp)).background(Color(0xFF6EE7B7)))
                            Box(Modifier.width(4.dp).height(h4).clip(RoundedCornerShape(2.dp)).background(Color(0xFF34D399)))
                            Box(Modifier.width(4.dp).height(h5).clip(RoundedCornerShape(2.dp)).background(Color(0xFF10B981)))
                        }
                    }
                }
            }

            Spacer(modifier = Modifier.height(16.dp))

            // 2. Floating Overlay Quick Setting Card
            Card(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(18.dp),
                colors = CardDefaults.cardColors(containerColor = Color(0xFF18181B))
            ) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 20.dp, vertical = 14.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text(
                            text = "Floating Mic Bubble",
                            fontWeight = FontWeight.SemiBold,
                            color = Color(0xFFF4F4F5),
                            fontSize = 15.sp
                        )
                        Text(
                            text = "Dictate over any app into your clipboard",
                            color = Color(0xFFA1A1AA),
                            fontSize = 12.sp
                        )
                    }
                    Switch(
                        checked = isOverlayActive,
                        onCheckedChange = { onToggleOverlay() },
                        colors = SwitchDefaults.colors(
                            checkedThumbColor = Color.White,
                            checkedTrackColor = Color(0xFF10B981)
                        )
                    )
                }
            }

            Spacer(modifier = Modifier.height(20.dp))

            // 3. Transcription History Header
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text(
                    text = "Recent Transcriptions",
                    fontWeight = FontWeight.SemiBold,
                    color = Color(0xFFE4E4E7),
                    fontSize = 16.sp
                )
                Text(
                    text = "${history.size} items",
                    color = Color(0xFF71717A),
                    fontSize = 12.sp
                )
            }

            Spacer(modifier = Modifier.height(10.dp))

            // 4. Transcription History List
            if (history.isEmpty()) {
                Box(
                    modifier = Modifier
                        .fillMaxWidth()
                        .weight(1f),
                    contentAlignment = Alignment.Center
                ) {
                    Text(
                        text = "No transcriptions yet.\nTap REC or use the floating bubble.",
                        color = Color(0xFF52525B),
                        fontSize = 13.sp,
                        textAlign = androidx.compose.ui.text.style.TextAlign.Center
                    )
                }
            } else {
                LazyColumn(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    items(history, key = { it.id }) { item ->
                        Card(
                            modifier = Modifier.fillMaxWidth(),
                            shape = RoundedCornerShape(14.dp),
                            colors = CardDefaults.cardColors(containerColor = Color(0xFF18181B))
                        ) {
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(14.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Text(
                                    text = item.text,
                                    color = Color(0xFFF4F4F5),
                                    fontSize = 14.sp,
                                    maxLines = 2,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.weight(1f)
                                )
                                Spacer(modifier = Modifier.width(10.dp))
                                Button(
                                    onClick = { onCopyItem(item.text) },
                                    colors = ButtonDefaults.buttonColors(
                                        containerColor = Color(0xFF27272A),
                                        contentColor = Color(0xFF34D399)
                                    ),
                                    contentPadding = PaddingValues(horizontal = 12.dp, vertical = 4.dp),
                                    shape = RoundedCornerShape(8.dp)
                                ) {
                                    Text("Copy", fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

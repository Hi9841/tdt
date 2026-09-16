package com.voicestt.mobile.engine

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlin.math.sqrt

class AudioRecordManager {

    private val sampleRate = 16000
    private val channelConfig = AudioFormat.CHANNEL_IN_MONO
    private val audioFormat = AudioFormat.ENCODING_PCM_16BIT

    private var audioRecord: AudioRecord? = null
    private var recordingJob: Job? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val maxSamples = sampleRate * 120

    private val _audioLevel = MutableStateFlow(0f)
    val audioLevel: StateFlow<Float> = _audioLevel.asStateFlow()

    private val audioBuffer = mutableListOf<Float>()
    private val bufferLock = Any()

    @SuppressLint("MissingPermission")
    fun startRecording(): Boolean {
        stopRecording()

        val minBufferSize = AudioRecord.getMinBufferSize(sampleRate, channelConfig, audioFormat)
        val bufferSize = maxOf(minBufferSize, sampleRate * 2)

        try {
            audioRecord = AudioRecord(
                MediaRecorder.AudioSource.MIC,
                sampleRate,
                channelConfig,
                audioFormat,
                bufferSize
            )

            if (audioRecord?.state != AudioRecord.STATE_INITIALIZED) {
                return false
            }

            synchronized(bufferLock) {
                audioBuffer.clear()
            }

            audioRecord?.startRecording()

            recordingJob = scope.launch {
                val shortBuffer = ShortArray(1024)
                while (isActive && audioRecord?.recordingState == AudioRecord.RECORDSTATE_RECORDING) {
                    val read = audioRecord?.read(shortBuffer, 0, shortBuffer.size) ?: -1
                    if (read > 0) {
                        var sumSq = 0.0
                        val floatChunk = FloatArray(read)
                        for (i in 0 until read) {
                            val sample = shortBuffer[i] / 32768.0f
                            floatChunk[i] = sample
                            sumSq += (sample * sample)
                        }

                        val rms = sqrt(sumSq / read).toFloat().coerceIn(0f, 1f)
                        _audioLevel.value = rms

                        synchronized(bufferLock) {
                            val room = (maxSamples - audioBuffer.size).coerceAtLeast(0)
                            for (i in 0 until minOf(read, room)) {
                                audioBuffer.add(floatChunk[i])
                            }
                        }
                    }
                }
            }
            return true
        } catch (e: Exception) {
            e.printStackTrace()
            return false
        }
    }

    fun stopRecording(): FloatArray {
        try {
            recordingJob?.cancel()
            recordingJob = null

            audioRecord?.stop()
            audioRecord?.release()
            audioRecord = null
            _audioLevel.value = 0f
        } catch (e: Exception) {
            e.printStackTrace()
        }

        return synchronized(bufferLock) {
            val result = audioBuffer.toFloatArray()
            audioBuffer.clear()
            result
        }
    }

    val isRecording: Boolean
        get() = audioRecord?.recordingState == AudioRecord.RECORDSTATE_RECORDING

    fun release() {
        stopRecording()
        scope.cancel()
    }
}

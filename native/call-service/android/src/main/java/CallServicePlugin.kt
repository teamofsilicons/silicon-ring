package com.teamofsilicons.callservice

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import org.json.JSONObject
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

// Native sessions are encrypted using a nonexportable Android Keystore key.
internal object SessionVault {
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey("ring-session", null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder("ring-session", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
    fun save(context: Context, value: JSONObject?) {
        val settings = context.getSharedPreferences("ring-native", Context.MODE_PRIVATE)
        if (value == null) { settings.edit().clear().apply(); return }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding"); cipher.init(Cipher.ENCRYPT_MODE, key())
        val encrypted = cipher.doFinal(value.toString().toByteArray(Charsets.UTF_8))
        settings.edit().putString("iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP)).putString("session", Base64.encodeToString(encrypted, Base64.NO_WRAP)).apply()
    }
    fun load(context: Context): JSONObject? = try {
        val settings = context.getSharedPreferences("ring-native", Context.MODE_PRIVATE)
        val encrypted = settings.getString("session", null)
        if (encrypted == null) null else {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.decode(settings.getString("iv", ""), Base64.NO_WRAP)))
            JSONObject(String(cipher.doFinal(Base64.decode(encrypted, Base64.NO_WRAP)), Charsets.UTF_8))
        }
    } catch (_: Exception) { null }
}

@TauriPlugin
class CallServicePlugin(private val activity: Activity) : Plugin(activity) {
    @Command fun configure(invoke: Invoke) {
        try {
            val payload = JSONObject(invoke.getArgs().toString())
            val runtime = RingRuntime.get(activity)
            runtime.configure(payload)
            val configured = FirebaseApp.initializeApp(activity) != null
            if (configured) FirebaseMessaging.getInstance().token.addOnSuccessListener { runtime.registerPush(it) }
            val permissions = mutableListOf<String>()
            if (ContextCompat.checkSelfPermission(activity, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED)
                permissions.add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= 33 && ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED)
                permissions.add(Manifest.permission.POST_NOTIFICATIONS)
            if (permissions.isNotEmpty()) activity.runOnUiThread { ActivityCompat.requestPermissions(activity, permissions.toTypedArray(), 7103) }
            invoke.resolve(JSObject().put("mobile", true).put("native_audio", true).put("platform", "android").put("push_configured", configured))
        } catch (error: Exception) { invoke.reject(error.message ?: "Native calling setup failed") }
    }
    @Command fun control(invoke: Invoke) {
        val payload = JSONObject(invoke.getArgs().toString())
        val runtime = RingRuntime.get(activity)
        val reply: (JSONObject?, String?) -> Unit = { value, error ->
            if (error != null) invoke.reject(error) else invoke.resolve(JSObject(value?.toString() ?: "{}"))
        }
        when (payload.optString("action")) {
            "start" -> {
                if (ContextCompat.checkSelfPermission(activity, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                    ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.RECORD_AUDIO), 7102)
                    invoke.reject("Allow microphone access, then reconnect your microphone."); return
                }
                if (Build.VERSION.SDK_INT >= 33 && ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED)
                    ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 7103)
                runtime.startAudio(payload.optString("ringid"), payload.optString("voicemail_id").ifEmpty { null }, reply)
            }
            "stop" -> runtime.stopAudio(reply)
            "mute" -> runtime.mute(payload.optBoolean("muted"), reply)
            "logout" -> { runtime.logout(); invoke.resolve() }
            "restore" -> reply(runtime.credentials, null)
            "status" -> reply(JSONObject().put("mobile", true).put("stream_id", runtime.audio.streamId).put("muted", runtime.audio.muted), null)
            else -> invoke.reject("Unknown native call action")
        }
    }
}

// §8 push, Android side: open a sealed notification with the stored push
// keys, push-pin list and push context. Used by the Expo module and by the
// messaging service (E9a). Any exception: post the constant placeholder.
package dev.dragent.e2ee

import org.json.JSONObject
import uniffi.dragent_e2ee_ffi.Keypair
import uniffi.dragent_e2ee_ffi.PushNotification
import uniffi.dragent_e2ee_ffi.PushPin
import uniffi.dragent_e2ee_ffi.decodePushPins
import uniffi.dragent_e2ee_ffi.openPush

object DragentE2EEPush {
  fun pushKeys(ks: DragentE2EEKeyStore, nowMs: Long): List<Keypair> {
    val keys = mutableListOf<Keypair>()
    ks.read(KeyItem.PUSH)?.let { cur ->
      try { keys.add(Keypair.fromSecret(cur)) } finally { cur.fill(0) }
    }
    ks.read(KeyItem.PUSH_PREVIOUS)?.let { prev ->
      try {
        val retired = DragentE2EEKeyStore.readU64(prev)
        if (prev.size == 40 && retired != null && nowMs >= retired &&
          nowMs - retired < DragentE2EEKeyStore.PREVIOUS_KEY_GRACE_MS
        ) {
          val secret = prev.copyOfRange(8, 40)
          try { keys.add(Keypair.fromSecret(secret)) } finally { secret.fill(0) }
        }
      } finally { prev.fill(0) }
    }
    return keys
  }

  fun pins(ks: DragentE2EEKeyStore): List<PushPin> = ks.read(KeyItem.PUSH_PINS)?.let { decodePushPins(it) } ?: emptyList()

  fun context(ks: DragentE2EEKeyStore): Pair<String, String>? = ks.read(KeyItem.PUSH_CONTEXT)?.let {
    val o = JSONObject(String(it))
    Pair(o.getString("account"), o.getString("phone"))
  }

  fun open(ks: DragentE2EEKeyStore, sealed: ByteArray, nowMs: Long): PushNotification {
    val (account, phone) = context(ks) ?: throw IllegalStateException("push context not set")
    return openPush(pushKeys(ks, nowMs), account, phone, sealed, pins(ks), nowMs.toULong())
  }
}

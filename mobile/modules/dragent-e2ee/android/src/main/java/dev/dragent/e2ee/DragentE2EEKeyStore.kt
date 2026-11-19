// Phone key storage for end-to-end encryption (docs/design/e2ee.md §6.1, §6.7, §8).
//
// Shared by the Expo module and the push messaging service (E9a): no Expo
// dependency. Every secret is stored wrapped by an AES-256-GCM key that never
// leaves the Android Keystore:
//
// | item               | wrap key                 | Keystore requirement (§6.7)                |
// |--------------------|--------------------------|--------------------------------------------|
// | identity.v1        | dragent.e2ee.wrap.id.v1  | setUnlockedDeviceRequired(true) (API 28+)  |
// | pinstore.key.v1    | dragent.e2ee.wrap.id.v1  | same                                       |
// | pinstore.gen.v1    | dragent.e2ee.wrap.id.v1  | same                                       |
// | push.v1            | dragent.e2ee.wrap.push.v1| none, so the service decrypts while locked |
// | push.previous.v1   | dragent.e2ee.wrap.push.v1| none                                       |
// | pushpins.v1        | dragent.e2ee.wrap.push.v1| none                                       |
// | pushcontext.v1     | dragent.e2ee.wrap.push.v1| none                                       |
//
// Wrapped blobs (iv || ciphertext+tag, AAD = item name) live in the private
// SharedPreferences file "dragent_e2ee". The app disables backup
// (app.json android.allowBackup = false), and Keystore keys are never backed
// up, so a restored copy could not be unwrapped anyway: that is "keys lost,
// pair again" (§9).
package dev.dragent.e2ee

import android.app.KeyguardManager
import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

enum class WrapClass(val alias: String, val unlockedDeviceRequired: Boolean) {
  /** §6.7 identity class: usable only while the device is unlocked. */
  IDENTITY("dragent.e2ee.wrap.id.v1", true),
  /** §6.7 push class: usable while locked (after first unlock). */
  PUSH("dragent.e2ee.wrap.push.v1", false),
}

enum class KeyItem(val account: String, val wrap: WrapClass) {
  IDENTITY("identity.v1", WrapClass.IDENTITY),
  PIN_STORE_KEY("pinstore.key.v1", WrapClass.IDENTITY),
  PIN_STORE_GENERATION("pinstore.gen.v1", WrapClass.IDENTITY),
  PUSH("push.v1", WrapClass.PUSH),
  PUSH_PREVIOUS("push.previous.v1", WrapClass.PUSH),
  PUSH_PINS("pushpins.v1", WrapClass.PUSH),
  PUSH_CONTEXT("pushcontext.v1", WrapClass.PUSH),
}

sealed class KeyStoreError(message: String) : Exception(message) {
  class NoPasscode : KeyStoreError("Set a passcode on this phone to pair it.")
  class Locked : KeyStoreError("The phone is locked.")
  class Unsupported : KeyStoreError("This Android version can't protect the keys (API 28+ needed).")
  class Corrupt(item: String) : KeyStoreError("Stored key $item couldn't be unwrapped.")
}

class DragentE2EEKeyStore(private val context: Context) {
  private val prefs = context.getSharedPreferences("dragent_e2ee", Context.MODE_PRIVATE)
  private val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

  fun hasDevicePasscode(): Boolean =
    (context.getSystemService(Context.KEYGUARD_SERVICE) as KeyguardManager).isDeviceSecure

  private fun wrapKey(cls: WrapClass): SecretKey {
    (keyStore.getKey(cls.alias, null) as? SecretKey)?.let { return it }
    if (cls.unlockedDeviceRequired && Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
      throw KeyStoreError.Unsupported()
    }
    val spec = KeyGenParameterSpec.Builder(cls.alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
      .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
      .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
      .setKeySize(256)
      .setRandomizedEncryptionRequired(true)
      .apply {
        if (cls.unlockedDeviceRequired && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
          setUnlockedDeviceRequired(true)
        }
      }
      .build()
    val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    gen.init(spec)
    return gen.generateKey()
  }

  private fun mapLocked(e: Exception): Exception =
    if (e is android.security.keystore.UserNotAuthenticatedException ||
      e.javaClass.simpleName == "KeyPermanentlyInvalidatedException" ||
      (Build.VERSION.SDK_INT >= 33 && e is android.security.KeyStoreException)
    ) KeyStoreError.Locked() else e

  fun read(item: KeyItem): ByteArray? {
    val stored = prefs.getString(item.account, null) ?: return null
    val blob = Base64.decode(stored, Base64.NO_WRAP)
    if (blob.size < 12 + 16) throw KeyStoreError.Corrupt(item.account)
    try {
      val cipher = Cipher.getInstance("AES/GCM/NoPadding")
      cipher.init(Cipher.DECRYPT_MODE, wrapKey(item.wrap), GCMParameterSpec(128, blob, 0, 12))
      cipher.updateAAD(item.account.toByteArray())
      return cipher.doFinal(blob, 12, blob.size - 12)
    } catch (e: javax.crypto.AEADBadTagException) {
      throw KeyStoreError.Corrupt(item.account)
    } catch (e: Exception) {
      throw mapLocked(e)
    }
  }

  fun write(item: KeyItem, value: ByteArray) {
    try {
      val cipher = Cipher.getInstance("AES/GCM/NoPadding")
      cipher.init(Cipher.ENCRYPT_MODE, wrapKey(item.wrap))
      cipher.updateAAD(item.account.toByteArray())
      val ct = cipher.doFinal(value)
      val blob = cipher.iv + ct
      // commit(), not apply(): the write is durable before we report success.
      if (!prefs.edit().putString(item.account, Base64.encodeToString(blob, Base64.NO_WRAP)).commit()) {
        throw IllegalStateException("couldn't persist ${item.account}")
      }
    } catch (e: KeyStoreError) {
      throw e
    } catch (e: Exception) {
      throw mapLocked(e)
    }
  }

  fun delete(item: KeyItem) {
    prefs.edit().remove(item.account).commit()
  }

  /** Every item gone, and both wrap keys deleted from the Keystore. */
  fun deleteAll() {
    val edit = prefs.edit()
    KeyItem.values().forEach { edit.remove(it.account) }
    edit.commit()
    WrapClass.values().forEach { if (keyStore.containsAlias(it.alias)) keyStore.deleteEntry(it.alias) }
  }

  /** What protects each stored item, read back from the Keystore. */
  fun storageInfo(): List<Map<String, Any?>> = KeyItem.values().filter { prefs.contains(it.account) }.map { item ->
    val key = keyStore.getKey(item.wrap.alias, null) as? SecretKey
    val info = key?.let {
      val f = javax.crypto.SecretKeyFactory.getInstance(it.algorithm, "AndroidKeyStore")
      f.getKeySpec(it, android.security.keystore.KeyInfo::class.java) as android.security.keystore.KeyInfo
    }
    mapOf(
      "item" to item.account,
      "keyClass" to if (item.wrap.unlockedDeviceRequired) "unlockedDeviceRequired" else "afterFirstUnlock",
      "wrapAlias" to item.wrap.alias,
      "insideSecureHardware" to (info?.let {
        if (Build.VERSION.SDK_INT >= 31) it.securityLevel != KeyProperties.SECURITY_LEVEL_SOFTWARE
        else @Suppress("DEPRECATION") it.isInsideSecureHardware
      }),
      "synchronizable" to false,
    )
  }

  companion object {
    /** §6.6: a retired push key opens notifications for 7 more days. */
    const val PREVIOUS_KEY_GRACE_MS: Long = 7L * 24 * 60 * 60 * 1000

    fun u64(v: Long): ByteArray = ByteArray(8) { i -> (v ushr (56 - 8 * i)).toByte() }
    fun readU64(b: ByteArray): Long? =
      if (b.size < 8) null else (0 until 8).fold(0L) { acc, i -> (acc shl 8) or (b[i].toLong() and 0xff) }
  }
}

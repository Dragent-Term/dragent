// The phone's end-to-end encryption module, Android side (docs/design/e2ee.md
// §3.2, E4). Mirrors ios/DragentE2EEModule.swift function for function.
// Private keys never cross into JS: the identity and push key pairs are Rust
// objects held here, unwrapped from the Keystore (DragentE2EEKeyStore.kt).
package dev.dragent.e2ee

import expo.modules.kotlin.exception.CodedException
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.records.Field
import expo.modules.kotlin.records.Record
import expo.modules.kotlin.sharedobjects.SharedRef
import org.json.JSONObject
import uniffi.dragent_e2ee_ffi.*
import java.io.File

class ChannelRef(ref: Channel) : SharedRef<Channel>(ref)
class KkHandshakeRef(ref: KkHandshake, val hs1: ByteArray) : SharedRef<KkHandshake>(ref) {
  var caps: List<String> = emptyList()
}
class PairingHandshakeRef(ref: PairingHandshake, val hs1: ByteArray) : SharedRef<PairingHandshake>(ref)
class ReassemblerRef(ref: Reassembler) : SharedRef<Reassembler>(ref)
class PartSessionRef(ref: PartSession) : SharedRef<PartSession>(ref)
class PinStoreRef(ref: PinStore) : SharedRef<PinStore>(ref)

// Byte fields inside a record arrive as number arrays: Expo on Android
// converts a Uint8Array only as a top-level argument, and one nested in an
// object reaches Kotlin as a map ("cannot be cast to byte[]"). The JS side
// (src/api.ts, androidRecords) sends these fields as plain arrays.
private fun List<Int>.bytes(): ByteArray = ByteArray(size) { i -> this[i].toByte() }

class ChannelIdsArg : Record {
  @Field var account: String = ""
  @Field var phone: String = ""
  @Field var desktop: String = ""
  @Field var chan: List<Int> = emptyList()
  fun rust() = ChannelIds(account, phone, desktop, chan.bytes())
}

class FrameArg : Record {
  @Field var kind: String = ""
  @Field var step: Int = 0
  @Field var purpose: String? = null
  @Field var chan: List<Int> = emptyList()
  @Field var peer: String? = null
  @Field var ctr: Double = 0.0
  @Field var body: List<Int> = emptyList()
}

class E2EEException(code: String, message: String) : CodedException(code, message, null)

private fun invalid(what: String) = E2EEException("E2EE_INVALID_ARGUMENT", what)

/** Runs [body], turning Rust and Keystore errors into coded JS exceptions. */
private inline fun <T> e2ee(body: () -> T): T = try {
  body()
} catch (e: E2EEException) {
  throw e
} catch (e: E2eeException) {
  throw E2EEException(errorCode(e), e.toString())
} catch (e: KeyStoreError.NoPasscode) {
  throw E2EEException("E2EE_NO_PASSCODE", e.message ?: "")
} catch (e: KeyStoreError.Locked) {
  throw E2EEException("E2EE_LOCKED", e.message ?: "")
} catch (e: KeyStoreError.Unsupported) {
  throw E2EEException("E2EE_UNSUPPORTED", e.message ?: "")
} catch (e: KeyStoreError.Corrupt) {
  throw E2EEException("E2EE_STORE_LOST", e.message ?: "")
} catch (e: Exception) {
  throw E2EEException("E2EE_INTERNAL", e.javaClass.simpleName)
}

private fun frameKindName(k: FrameKind) = when (k) { FrameKind.HS -> "hs"; FrameKind.DATA -> "data"; FrameKind.CLOSE -> "close" }
private fun purposeName(p: FramePurpose?) = when (p) { FramePurpose.COMPANION -> "companion"; FramePurpose.PAIRING -> "pairing"; null -> null }
private fun parsePurpose(s: String?) = when (s) { null -> null; "companion" -> FramePurpose.COMPANION; "pairing" -> FramePurpose.PAIRING; else -> throw invalid("purpose") }
private fun parseKind(s: String) = when (s) { "hs" -> FrameKind.HS; "data" -> FrameKind.DATA; "close" -> FrameKind.CLOSE; else -> throw invalid("kind") }
private fun mtypeName(m: MessageType) = when (m) { MessageType.JSON -> "json"; MessageType.TERM -> "term"; MessageType.CTRL -> "ctrl" }
private fun parseMtype(s: String) = when (s) { "json" -> MessageType.JSON; "term" -> MessageType.TERM; "ctrl" -> MessageType.CTRL; else -> throw invalid("mtype") }
private fun parseClose(s: String) = when (s) {
  "local" -> LocalCloseReason.LOCAL; "peer_closed" -> LocalCloseReason.PEER_CLOSED
  "revoked" -> LocalCloseReason.REVOKED; "rekeyed" -> LocalCloseReason.REKEYED; else -> throw invalid("reason")
}
private fun stateObject(s: ChannelStateKind): Map<String, Any?> = when (s) {
  is ChannelStateKind.AwaitingConfirm -> mapOf("state" to "awaiting_confirm")
  is ChannelStateKind.Confirming -> mapOf("state" to "confirming")
  is ChannelStateKind.Open -> mapOf("state" to "open")
  is ChannelStateKind.WindingDown -> mapOf("state" to "winding_down")
  is ChannelStateKind.Closed -> mapOf("state" to "closed", "reason" to s.reason)
}
private fun pushObject(n: PushNotification): Map<String, Any?> = mapOf(
  "desktopId" to n.desktopId, "id" to n.id, "issuedAtMs" to n.issuedAtMs.toDouble(), "kind" to n.kind.toInt(),
  "dismiss" to n.dismiss, "title" to n.title, "body" to n.body, "workspaceId" to n.workspaceId,
  "tabId" to n.tabId, "dismissIds" to n.dismissIds,
)

class DragentE2EEModule : Module() {
  private val keyStore by lazy { DragentE2EEKeyStore(appContext.reactContext ?: throw IllegalStateException("no context")) }
  private val lock = Any()
  private var identityCache: Keypair? = null

  private fun identity(): Keypair = synchronized(lock) {
    identityCache ?: run {
      val secret = keyStore.read(KeyItem.IDENTITY)
        ?: throw E2EEException("E2EE_NO_IDENTITY", "No identity key on this phone; pair again.")
      try { Keypair.fromSecret(secret) } finally { secret.fill(0) }.also { identityCache = it }
    }
  }

  private fun store(item: KeyItem, k: Keypair) {
    val secret = k.secretBytes()
    try { keyStore.write(item, secret) } finally { secret.fill(0) }
  }

  private fun pushPublic(): ByteArray? = keyStore.read(KeyItem.PUSH)?.let { s ->
    try { Keypair.fromSecret(s).publicKey() } finally { s.fill(0) }
  }

  private fun createPush(): ByteArray = pushPublic() ?: Keypair.generate().also { store(KeyItem.PUSH, it) }.publicKey()

  private fun pinStoreFile(): File {
    val dir = File(appContext.reactContext!!.filesDir, "e2ee")
    dir.mkdirs()
    return File(dir, "state.v1")
  }

  private fun readGeneration(): Long = keyStore.read(KeyItem.PIN_STORE_GENERATION)?.let {
    DragentE2EEKeyStore.readU64(it) ?: throw E2EEException("E2EE_STORE_LOST", "generation")
  } ?: 0L

  private fun writeGeneration(g: Long) = keyStore.write(KeyItem.PIN_STORE_GENERATION, DragentE2EEKeyStore.u64(g))

  private fun openStore(): PinStore {
    val gen = readGeneration()
    val key = keyStore.read(KeyItem.PIN_STORE_KEY) ?: run {
      // No state key: only a brand-new store (generation 0, no file) may start fresh.
      if (gen != 0L) throw E2EEException("E2EE_STORE_LOST", "missing state key")
      generateStateKey().also { keyStore.write(KeyItem.PIN_STORE_KEY, it) }
    }
    try {
      val opened = openPinStore(pinStoreFile().path, key, gen.toULong())
      opened.bumpKeychainTo?.let { writeGeneration(it.toLong()) }
      return opened.store
    } finally {
      key.fill(0)
    }
  }

  private fun resetStore() {
    pinStoreFile().delete()
    keyStore.delete(KeyItem.PIN_STORE_KEY)
    keyStore.delete(KeyItem.PIN_STORE_GENERATION)
  }

  private fun readPins(): List<PushPin> = DragentE2EEPush.pins(keyStore)
  private fun writePins(pins: List<PushPin>) = keyStore.write(KeyItem.PUSH_PINS, encodePushPins(pins))

  override fun definition() = ModuleDefinition {
    Name("DragentE2EE")

    // A push decrypted while the app is open: shown as the app's own banner.
    Events("onForegroundPush")
    OnCreate {
      DragentForegroundPush.listener = { title, body, desktop ->
        sendEvent("onForegroundPush", mapOf("title" to title, "body" to body, "desktop" to desktop))
      }
    }
    OnDestroy { DragentForegroundPush.listener = null }

    Constant("platform") { "android" }
    Constant("vectorsCompiled") { bindingInfo().vectorsCompiled }

    // Device and keys (§6.1, §6.7)
    Function("hasDevicePasscode") { keyStore.hasDevicePasscode() }
    Function("identityPublicKey") {
      e2ee { if (keyStore.read(KeyItem.IDENTITY) == null) null else identity().publicKey() }
    }
    Function("createIdentityKey") {
      e2ee {
        if (keyStore.read(KeyItem.IDENTITY) != null) return@e2ee identity().publicKey()
        if (!keyStore.hasDevicePasscode()) throw KeyStoreError.NoPasscode()
        val k = Keypair.generate()
        store(KeyItem.IDENTITY, k)
        synchronized(lock) { identityCache = k }
        k.publicKey()
      }
    }
    Function("pushPublicKey") { e2ee { pushPublic() } }
    Function("createPushKey") { e2ee { createPush() } }
    Function("rotatePushKey") { nowMs: Double ->
      e2ee {
        keyStore.read(KeyItem.PUSH)?.let { old ->
          val prev = DragentE2EEKeyStore.u64(nowMs.toLong()) + old
          try { keyStore.write(KeyItem.PUSH_PREVIOUS, prev) } finally { old.fill(0); prev.fill(0) }
        }
        Keypair.generate().also { store(KeyItem.PUSH, it) }.publicKey()
      }
    }
    Function("deleteAllKeys") {
      e2ee {
        synchronized(lock) { identityCache = null }
        resetStore()
        keyStore.deleteAll()
      }
    }
    Function("keyStorageInfo") { e2ee { keyStore.storageInfo() } }

    // Push pins and push (§8)
    Function("setPushContext") { account: String, phone: String ->
      e2ee { keyStore.write(KeyItem.PUSH_CONTEXT, JSONObject(mapOf("account" to account, "phone" to phone)).toString().toByteArray()) }
    }
    Function("listPushPins") { e2ee { readPins().map { mapOf("desktop" to it.desktop, "key" to it.key) } } }
    Function("setPushPin") { desktop: String, key: ByteArray ->
      e2ee { writePins(readPins().filter { it.desktop != desktop && !it.key.contentEquals(key) } + PushPin(desktop, key)) }
    }
    Function("removePushPin") { desktop: String -> e2ee { writePins(readPins().filter { it.desktop != desktop }) } }
    Function("openPush") { sealed: ByteArray, nowMs: Double ->
      e2ee { pushObject(DragentE2EEPush.open(keyStore, sealed, nowMs.toLong())) }
    }

    // Pin store (§6.1)
    Function("openPinStore") { e2ee { PinStoreRef(openStore()) } }
    Function("resetPinStore") { e2ee { resetStore() } }

    Class("PinStoreRef", PinStoreRef::class) {
      Function("generation") { s: PinStoreRef -> s.ref.generation().toDouble() }
      Function("desktops") { s: PinStoreRef ->
        s.ref.desktops().map {
          mapOf("id" to it.id, "identityKey" to it.identityKey, "name" to it.name,
            "pinnedAtMs" to it.pinnedAtMs.toDouble(), "rotationKey" to it.rotationKey)
        }
      }
      Function("lookup") { s: PinStoreRef, device: String ->
        e2ee {
          when (val st = s.ref.lookup(device)) {
            is PinState.Pinned -> mapOf("state" to "pinned", "identityKey" to st.identityKey)
            is PinState.Unpinned -> mapOf("state" to "unpinned")
            is PinState.Tombstoned -> mapOf("state" to "tombstoned")
          }
        }
      }
      Function("candidateKeys") { s: PinStoreRef, device: String, nowMs: Double ->
        e2ee { s.ref.candidateKeys(device, nowMs.toLong().toULong()) }
      }
      Function("pinDesktop") { s: PinStoreRef, device: String, key: ByteArray, name: String, catalogVersion: Double, nowMs: Double ->
        e2ee { s.ref.pinDesktop(device, key, name, catalogVersion.toLong().toULong(), nowMs.toLong().toULong()) }
      }
      Function("announceRotation") { s: PinStoreRef, device: String, key: ByteArray, nowMs: Double ->
        e2ee { s.ref.announceRotation(device, key, nowMs.toLong().toULong()) }
      }
      Function("confirmRotation") { s: PinStoreRef, device: String, key: ByteArray -> e2ee { s.ref.confirmRotation(device, key) } }
      Function("revoke") { s: PinStoreRef, device: String, nowMs: Double -> e2ee { s.ref.revoke(device, nowMs.toLong().toULong()) } }
      Function("openCatalog") { s: PinStoreRef, account: String, phone: String, desktop: String, blob: ByteArray ->
        e2ee {
          when (val r = s.ref.openCatalog(identity(), account, phone, desktop, blob)) {
            is CatalogResult.Accepted -> mapOf("status" to "accepted", "version" to r.version.toDouble(), "json" to r.json)
            is CatalogResult.Unchanged -> mapOf("status" to "unchanged", "version" to r.version.toDouble())
          }
        }
      }
      Function("catalogHighWater") { s: PinStoreRef, desktop: String -> e2ee { s.ref.catalogHighWater(desktop)?.toDouble() } }
      Function("commit") { s: PinStoreRef ->
        e2ee { s.ref.commit().also { writeGeneration(it.toLong()) }.toDouble() }
      }
    }

    // Handshakes and channels (§4.2, §6.2)
    Function("startPairing") { ids: ChannelIdsArg, desktopKey: ByteArray, secretP: ByteArray, deviceName: String ->
      try {
        e2ee {
          val start = startPairing(identity(), createPush(), desktopKey, secretP, ids.rust(), deviceName, PhonePlatform.ANDROID)
          PairingHandshakeRef(start.handshake, start.hs1)
        }
      } finally {
        secretP.fill(0)
      }
    }
    Class("PairingHandshakeRef", PairingHandshakeRef::class) {
      Property("hs1") { h: PairingHandshakeRef -> h.hs1 }
      Function("finish") { h: PairingHandshakeRef, hs2: ByteArray, nowMs: Double ->
        e2ee { ChannelRef(h.ref.finish(hs2, nowMs.toLong().toULong())) }
      }
      Function("abandon") { h: PairingHandshakeRef -> h.ref.abandon() }
    }

    Function("startKk") { ids: ChannelIdsArg, desktopPin: ByteArray, caps: List<String> ->
      e2ee {
        val start = startKk(identity(), desktopPin, ids.rust(), caps)
        KkHandshakeRef(start.handshake, start.hs1)
      }
    }
    Class("KkHandshakeRef", KkHandshakeRef::class) {
      Property("hs1") { h: KkHandshakeRef -> h.hs1 }
      Property("peerCaps") { h: KkHandshakeRef -> h.caps }
      Function("finish") { h: KkHandshakeRef, hs2: ByteArray, nowMs: Double ->
        e2ee {
          val done = h.ref.finish(hs2, nowMs.toLong().toULong())
          h.caps = done.caps
          ChannelRef(done.channel)
        }
      }
      Function("abandon") { h: KkHandshakeRef -> h.ref.abandon() }
    }

    Class("ChannelRef", ChannelRef::class) {
      Property("chanId") { c: ChannelRef -> c.ref.chanId() }
      Function("state") { c: ChannelRef -> stateObject(c.ref.state()) }
      Function("handshakeHash") { c: ChannelRef -> c.ref.handshakeHash() }
      Function("remoteStatic") { c: ChannelRef -> c.ref.remoteStatic() }
      Function("hasKeys") { c: ChannelRef -> c.ref.hasKeys() }
      Function("processedThrough") { c: ChannelRef -> c.ref.processedThrough()?.toDouble() }
      Function("lastSentCtr") { c: ChannelRef -> c.ref.lastSentCtr()?.toDouble() }
      Function("isQuiesced") { c: ChannelRef -> c.ref.isQuiesced() }
      Function("seal") { c: ChannelRef, record: ByteArray ->
        e2ee { c.ref.seal(record).let { mapOf("ctr" to it.ctr.toDouble(), "ciphertext" to it.ciphertext) } }
      }
      Function("sealFrame") { c: ChannelRef, record: ByteArray -> e2ee { c.ref.sealFrame(record) } }
      Function("receive") { c: ChannelRef, ctr: Double, ciphertext: ByteArray ->
        e2ee {
          when (val r = c.ref.receive(ctr.toLong().toULong(), ciphertext)) {
            is Inbound.Record -> mapOf("kind" to "record", "record" to r.record)
            is Inbound.Confirming -> mapOf("kind" to "confirming", "record" to r.record)
            is Inbound.Duplicate -> mapOf("kind" to "duplicate")
          }
        }
      }
      Function("quiesce") { c: ChannelRef ->
        c.ref.quiesce().let { mapOf("chan" to it.chan, "lastSentCtr" to it.lastSentCtr?.toDouble()) }
      }
      Function("poll") { c: ChannelRef, nowMs: Double -> c.ref.poll(nowMs.toLong().toULong()) }
      Function("rekeyDue") { c: ChannelRef, nowMs: Double -> c.ref.rekeyDue(nowMs.toLong().toULong()) }
      Function("close") { c: ChannelRef, reason: String -> e2ee { c.ref.close(parseClose(reason)) } }
    }

    // Codecs (§4.1, §4.4, §4.5)
    Function("randomChannelId") { e2ee { randomChannelId() } }
    Function("randomBytes") { n: Int -> e2ee { randomBytes(n.coerceAtLeast(0).toUInt()) } }
    Function("sha256") { data: ByteArray -> sha256(data) }
    Function("decodeFrame") { bytes: ByteArray ->
      e2ee {
        val f = decodeFrame(bytes)
        mapOf("kind" to frameKindName(f.kind), "step" to f.step.toInt(), "purpose" to purposeName(f.purpose),
          "chan" to f.chan, "peer" to f.peer, "ctr" to f.ctr.toDouble(), "body" to f.body)
      }
    }
    Function("encodeFrame") { f: FrameArg ->
      e2ee {
        encodeFrame(Frame(parseKind(f.kind), f.step.coerceIn(0, 255).toUByte(), parsePurpose(f.purpose),
          f.chan.bytes(), f.peer, f.ctr.toLong().toULong(), f.body.bytes()))
      }
    }
    Function("encodeHs1Frame") { purpose: String, chan: ByteArray, peer: String, body: ByteArray ->
      e2ee { encodeHs1Frame(parsePurpose(purpose) ?: throw invalid("purpose"), chan, peer, body) }
    }
    Function("encodeRecords") { mtype: String, msgId: Double, content: ByteArray ->
      e2ee { encodeRecords(parseMtype(mtype), msgId.toLong().toUInt(), content) }
    }
    Function("encodePadRecord") { len: Int -> e2ee { encodePadRecord(len.coerceAtLeast(0).toUInt()) } }
    Function("decodeTerm") { content: ByteArray ->
      e2ee {
        val t = decodeTerm(content)
        mapOf("handle" to t.handle.toDouble(), "seq" to t.seq.toDouble(), "base" to t.base.toDouble(),
          "total" to t.total.toDouble(), "alive" to t.alive, "reset" to t.reset, "exitCode" to t.exitCode,
          "rows" to t.rows?.toInt(), "cols" to t.cols?.toInt(), "data" to t.data)
      }
    }
    Function("decodeCtrl") { content: ByteArray -> e2ee { decodeCtrl(content).let { mapOf("name" to it.name, "json" to it.json) } } }
    Function("encodeCtrl") { name: String, json: String -> e2ee { encodeCtrl(name, json) } }

    Function("createReassembler") { ReassemblerRef(Reassembler()) }
    Class("ReassemblerRef", ReassemblerRef::class) {
      Function("feed") { r: ReassemblerRef, record: ByteArray ->
        e2ee { r.ref.feed(record)?.let { mapOf("mtype" to mtypeName(it.mtype), "content" to it.content) } }
      }
      Function("buffered") { r: ReassemblerRef -> r.ref.buffered().toDouble() }
      Function("clear") { r: ReassemblerRef -> r.ref.clear() }
    }

    Function("beginPart") { ctrlContent: ByteArray -> e2ee { PartSessionRef(PartSession.begin(ctrlContent)) } }
    Class("PartSessionRef", PartSessionRef::class) {
      Function("info") { p: PartSessionRef ->
        val i = p.ref.info()
        mapOf("partId" to i.partId, "purpose" to i.purpose.name.lowercase(), "requestIdJson" to i.requestIdJson,
          "streamHandle" to i.streamHandle?.toDouble(), "totalBytes" to i.totalBytes.toDouble(),
          "partCount" to i.partCount.toDouble())
      }
      Function("feed") { p: PartSessionRef, chunkJson: ByteArray -> e2ee { p.ref.feed(chunkJson) } }
      Function("end") { p: PartSessionRef, ctrlContent: ByteArray -> e2ee { p.ref.end(ctrlContent) } }
      Function("abort") { p: PartSessionRef -> p.ref.abort() }
    }

    // Benchmarks (§11, §12.12)
    Function("nativeBenchmarks") { iterations: Int ->
      e2ee {
        runNativeBenchmarks(iterations.coerceAtLeast(1).toUInt()).map {
          mapOf("name" to it.name, "iterations" to it.iterations.toDouble(), "microsPerOp" to it.microsPerOp)
        }
      }
    }
  }
}

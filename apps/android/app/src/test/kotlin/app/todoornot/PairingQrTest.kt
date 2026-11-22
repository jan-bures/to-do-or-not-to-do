package app.todoornot

import androidx.compose.ui.graphics.toArgb
import app.todoornot.core.pairingQr
import app.todoornot.design.Design
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import org.junit.Assert.assertEquals
import org.junit.Test

/** The decoder used by the camera reads both kinds of tile from the shared Rust encoder. */
class PairingQrTest {
    @Test
    fun bothLinksRoundTrip() {
        val urls = listOf(
            "https://todo.example.com/pair?code=ABCD-EFGH",
            "https://todo.example.com/pair/invite#" + "A".repeat(43),
        )
        val colors = Design(false, "my-day").room
        val ink = colors.ink.toArgb()
        val pane = colors.pane.toArgb()
        for (url in urls) {
            val qr = requireNotNull(pairingQr(url))
            val modules = qr.width.toInt()
            val scale = 4
            val side = modules * scale
            val pixels = IntArray(side * side) { pixel ->
                val col = pixel % side / scale
                val row = pixel / side / scale
                if (qr.dark[row * modules + col]) ink else pane
            }
            val bitmap = BinaryBitmap(HybridBinarizer(RGBLuminanceSource(side, side, pixels)))
            assertEquals(url, QRCodeReader().decode(bitmap, mapOf(DecodeHintType.PURE_BARCODE to true)).text)
        }
    }
}

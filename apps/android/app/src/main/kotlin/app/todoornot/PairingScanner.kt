package app.todoornot

import android.Manifest
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import app.todoornot.model.PairingScan
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions

/** Registered at Activity creation, independent of which screen or dialog is composed. */
internal class PairingScanner(owner: ComponentActivity, private val requests: () -> PairingScan) {
    private val scanner = owner.registerForActivityResult(ScanContract()) { result ->
        requests().result(result.contents)
    }
    private val permission = owner.registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (requests().permission(granted)) {
            scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setBeepEnabled(false).setOrientationLocked(false))
        }
    }

    fun launch(read: (String) -> Unit, denied: () -> Unit) {
        if (requests().begin(read, denied)) permission.launch(Manifest.permission.CAMERA)
    }
}

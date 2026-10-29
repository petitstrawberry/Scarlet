package dev.scarlet.compat;

import android.content.res.Resources;
import android.os.Bundle;
import android.os.Parcel;

/** Executed through the APK's own DexClassLoader before Activity launch exists. */
public final class RuntimePayload {
    public static String run(Resources resources) throws Exception {
        int number = resources.getIdentifier("probe_answer", "integer", "dev.scarlet.compat");
        int message = resources.getIdentifier("probe_message", "string", "dev.scarlet.compat");
        if (number == 0 || message == 0) throw new AssertionError("APK resource identifiers");
        Bundle bundle = new Bundle();
        bundle.putInt("answer", resources.getInteger(number));
        bundle.putString("message", resources.getString(message));
        Parcel parcel = Parcel.obtain();
        try {
            parcel.writeBundle(bundle);
            parcel.setDataPosition(0);
            Bundle decoded = parcel.readBundle(RuntimePayload.class.getClassLoader());
            if (decoded.getInt("answer") != 42) throw new AssertionError("APK payload Bundle");
            return decoded.getString("message");
        } finally { parcel.recycle(); }
    }
}

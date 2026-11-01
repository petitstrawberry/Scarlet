package com.android.systemui.compat;

import android.content.Context;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.util.Log;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;

/** Restores PowerManager access when stock SystemUI's service cache is empty. */
public final class PowerManagerCompat {
    private static final String TAG = "ScarletSystemUI";

    private PowerManagerCompat() {}

    public static PowerManager get(Context context) {
        PowerManager manager = (PowerManager) context.getSystemService(PowerManager.class);
        if (manager != null) return manager;
        try {
            Class<?> serviceManager = Class.forName("android.os.ServiceManager");
            Method getService = serviceManager.getMethod("getService", String.class);
            IBinder binder = (IBinder) getService.invoke(null, Context.POWER_SERVICE);
            if (binder == null) {
                Log.e(TAG, "PowerManager service binder is absent");
                return null;
            }
            Class<?> powerInterface = Class.forName("android.os.IPowerManager");
            Class<?> powerStub = Class.forName("android.os.IPowerManager$Stub");
            Object service = powerStub.getMethod("asInterface", IBinder.class).invoke(null, binder);
            IBinder thermalBinder = (IBinder) getService.invoke(null, "thermalservice");
            Object thermalService = null;
            Class<?> thermalInterface = null;
            if (thermalBinder != null) {
                thermalInterface = Class.forName("android.os.IThermalService");
                Class<?> thermalStub = Class.forName("android.os.IThermalService$Stub");
                thermalService = thermalStub.getMethod("asInterface", IBinder.class)
                        .invoke(null, thermalBinder);
            } else {
                Log.w(TAG, "ThermalManager service is absent; using the PowerManager service only");
                thermalInterface = Class.forName("android.os.IThermalService");
            }
            Constructor<PowerManager> constructor = PowerManager.class.getDeclaredConstructor(
                    Context.class, powerInterface, thermalInterface, Handler.class);
            constructor.setAccessible(true);
            manager = constructor.newInstance(
                    context, service, thermalService, new Handler(Looper.getMainLooper()));
            Log.i(TAG, "PowerManager restored from the registered system service");
            return manager;
        } catch (Throwable error) {
            Log.e(TAG, "PowerManager compatibility lookup failed", error);
            return null;
        }
    }
}

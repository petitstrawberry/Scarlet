import android.app.Activity;
import android.content.res.AssetManager;
import android.content.res.Configuration;
import android.content.res.Resources;
import android.content.res.XmlResourceParser;
import android.util.DisplayMetrics;
import dalvik.system.DexClassLoader;
import java.io.File;
import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.PrintStream;
import java.lang.reflect.Method;

/** Stock package parser, resource JNI and APK DEX loading; no Activity launch. */
public final class ScarletApkProbe {
    private static final String APK = "/data/local/tmp/scarlet-compat.apk";
    private static final PrintStream out = new PrintStream(new FileOutputStream(FileDescriptor.out));
    private static void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
    private static Object field(Object value, String name) throws Exception {
        return value.getClass().getField(name).get(value);
    }
    private static void run() throws Exception {
        out.println("SCARLET_APK_RUNTIME_BEGIN");
        Class<?> parserClass = Class.forName("android.content.pm.PackageParser");
        Method parse = parserClass.getMethod("parseApkLite", File.class, int.class);
        Object parsed = parse.invoke(null, new File(APK), 0);
        require("dev.scarlet.compat".equals(field(parsed, "packageName")), "parsed package identity");
        require((Integer) field(parsed, "minSdkVersion") == 29 && (Integer) field(parsed, "targetSdkVersion") == 32, "parsed SDK levels");
        out.println("SCARLET_APK_RUNTIME_PACKAGE_LITE_OK");
        Class<?> inputClass = Class.forName("android.content.pm.parsing.result.ParseInput");
        Class<?> resultClass = Class.forName("android.content.pm.parsing.result.ParseResult");
        Method input = Class.forName("android.content.pm.parsing.result.ParseTypeImpl").getMethod("forParsingWithoutPlatformCompat");
        Method verify = Class.forName("android.util.apk.ApkSignatureVerifier").getMethod("verify", inputClass, String.class, int.class);
        // Use Android's archive-inspection API with full signature verification;
        // no platform-compat service or certificate-only/unsafe entry point.
        Object verified = verify.invoke(null, input.invoke(null), APK, 2);
        require(!(Boolean) resultClass.getMethod("isError").invoke(verified),
            "verified APK signature: " + resultClass.getMethod("getErrorMessage").invoke(verified));
        Object signing = resultClass.getMethod("getResult").invoke(verified);
        Object signatures = signing.getClass().getMethod("getSignatures").invoke(signing);
        require(signatures != null && java.lang.reflect.Array.getLength(signatures) == 1, "verified APK signing certificate");
        out.println("SCARLET_APK_RUNTIME_SIGNATURE_OK");
        Object rejected = verify.invoke(null, input.invoke(null), "/data/local/tmp/scarlet-tampered.apk", 2);
        require((Boolean) resultClass.getMethod("isError").invoke(rejected)
            && (Integer) resultClass.getMethod("getErrorCode").invoke(rejected) == -103,
            "APK signature rejects changed signed content");
        out.println("SCARLET_APK_RUNTIME_TAMPER_REJECTED");
        AssetManager assets = AssetManager.class.getConstructor().newInstance();
        int cookie = (Integer) AssetManager.class.getMethod("addAssetPath", String.class).invoke(assets, APK);
        require(cookie != 0, "APK asset path");
        DisplayMetrics metrics = new DisplayMetrics();
        metrics.setToDefaults();
        Configuration configuration = new Configuration();
        configuration.setToDefaults();
        Resources resources = new Resources(assets, metrics, configuration);
        int label = 0;
        String activityName = null;
        boolean exported = false;
        String namespace = "http://schemas.android.com/apk/res/android";
        try (XmlResourceParser manifest = (XmlResourceParser) AssetManager.class
                .getMethod("openXmlResourceParser", int.class, String.class).invoke(assets, cookie, "AndroidManifest.xml")) {
            while (manifest.next() != XmlResourceParser.END_DOCUMENT) {
                if (manifest.getEventType() != XmlResourceParser.START_TAG) continue;
                if ("application".equals(manifest.getName())) label = manifest.getAttributeResourceValue(namespace, "label", 0);
                if ("activity".equals(manifest.getName())) {
                    require(activityName == null, "single manifest Activity");
                    activityName = manifest.getAttributeValue(namespace, "name");
                    exported = manifest.getAttributeBooleanValue(namespace, "exported", false);
                }
            }
        }
        require(".MainActivity".equals(activityName) && exported, "compiled manifest Activity declaration");
        require(label != 0 && "Scarlet compatibility probe".equals(resources.getString(label)), "APK resource application label");
        require(resources.getString(android.R.string.ok).length() > 0, "stock framework resources");
        try (InputStream stream = assets.open("probe.txt")) {
            byte[] bytes = new byte[64];
            int count = stream.read(bytes);
            require(count > 0 && "Scarlet APK asset\n".equals(new String(bytes, 0, count, "UTF-8")) && stream.read() == -1, "APK asset contents");
        }
        out.println("SCARLET_APK_RUNTIME_RESOURCES_OK");
        require(new File("/data/local/tmp/apk-cache").mkdir(), "APK cache directory");
        ClassLoader loader = new DexClassLoader(APK, "/data/local/tmp/apk-cache", null, ScarletApkProbe.class.getClassLoader());
        Class<?> payload = loader.loadClass("dev.scarlet.compat.RuntimePayload");
        require(payload.getClassLoader() == loader, "payload comes from APK class loader");
        require("Android resources on Scarlet".equals(payload.getMethod("run", Resources.class).invoke(null, resources)), "executed APK code with framework Resources and Parcel");
        Class<?> component = loader.loadClass("dev.scarlet.compat" + activityName);
        require(component.getClassLoader() == loader && Activity.class.isAssignableFrom(component), "APK Activity class linkage");
        assets.close();
        out.println("SCARLET_APK_RUNTIME_DEX_OK");
        out.println("SCARLET_APK_RUNTIME_OK");
    }
    public static void main(String[] args) throws Exception {
        try { run(); }
        catch (Throwable error) {
            out.println("SCARLET_APK_RUNTIME_FAIL");
            error.printStackTrace(out);
            out.flush();
            if (error instanceof Exception) throw (Exception) error;
            throw new AssertionError(error);
        }
    }
}

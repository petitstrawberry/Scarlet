package dev.scarlet.compat;

import android.app.Activity;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.Typeface;
import android.os.Bundle;
import android.util.Log;
import android.view.MotionEvent;
import android.view.View;

public class MainActivity extends Activity {
    private String run;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        run = getIntent().getStringExtra("scarlet_run");
        setContentView(new DashboardView());
    }

    private final class DashboardView extends View {
        private static final int ACCENT = 0xFF2DD4BF;
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF rect = new RectF();
        private int touches;

        DashboardView() {
            super(MainActivity.this);
            setBackgroundColor(Color.TRANSPARENT);
        }

        @Override protected void onDraw(Canvas canvas) {
            float w = getWidth();
            float h = getHeight();
            paint.setShader(new LinearGradient(0, 0, w, h,
                    0xFF08111F, 0xFF14213A, Shader.TileMode.CLAMP));
            canvas.drawRect(0, 0, w, h, paint);
            paint.setShader(null);

            paint.setColor(withAlpha(ACCENT, 38));
            canvas.drawCircle(w * 0.86f, h * 0.16f, w * 0.30f, paint);
            paint.setColor(withAlpha(ACCENT, 22));
            canvas.drawCircle(w * 0.10f, h * 0.90f, w * 0.24f, paint);

            float left = 34;
            float top = 30;
            paint.setTextSize(13);
            paint.setTypeface(Typeface.create("sans", Typeface.BOLD));
            paint.setLetterSpacing(0.12f);
            paint.setColor(0xFF8EA3C2);
            canvas.drawText("SCARLET  /  ANDROID COMPATIBILITY", left, top, paint);

            paint.setLetterSpacing(0);
            paint.setTypeface(Typeface.create("sans", Typeface.BOLD));
            paint.setColor(Color.WHITE);
            paint.setTextSize(38);
            canvas.drawText("Scarlet", left, top + 60, paint);
            paint.setTypeface(Typeface.create("sans", Typeface.NORMAL));
            paint.setColor(0xFFB6C4D8);
            paint.setTextSize(16);
            canvas.drawText("A native Android activity running on Scarlet", left, top + 88, paint);

            rect.set(left, top + 116, w - left, top + 260);
            paint.setColor(0xCC14243C);
            canvas.drawRoundRect(rect, 24, 24, paint);
            paint.setColor(withAlpha(ACCENT, 72));
            rect.set(left + 20, top + 138, w - left - 20, top + 202);
            canvas.drawRoundRect(rect, 18, 18, paint);
            paint.setColor(ACCENT);
            rect.set(left + 20, top + 138, left + 34, top + 202);
            canvas.drawRoundRect(rect, 8, 8, paint);

            paint.setColor(Color.WHITE);
            paint.setTextSize(22);
            paint.setTypeface(Typeface.create("sans", Typeface.BOLD));
            canvas.drawText("System online", left + 50, top + 174, paint);
            paint.setTypeface(Typeface.create("sans", Typeface.NORMAL));
            paint.setTextSize(14);
            paint.setColor(0xFFC6D3E4);
            canvas.drawText("Bionic  ·  ART  ·  Binder  ·  SWS", left + 50, top + 197, paint);

            float chipY = top + 222;
            drawChip(canvas, left + 20, chipY, "ANDROID 13", 0xFF67E8F9);
            drawChip(canvas, left + 154, chipY, "ARM64", 0xFFF9A8D4);
            drawChip(canvas, left + 244, chipY, "CPU RENDER", 0xFFFDE68A);

            rect.set(left, h - 108, w - left, h - 38);
            paint.setColor(0xAA0B1728);
            canvas.drawRoundRect(rect, 22, 22, paint);
            paint.setColor(Color.WHITE);
            paint.setTextSize(15);
            paint.setTypeface(Typeface.create("sans", Typeface.BOLD));
            canvas.drawText(touches == 0 ? "Tap anywhere to test input"
                    : "Input received  x  " + touches, left + 22, h - 66, paint);
            paint.setColor(ACCENT);
            canvas.drawCircle(w - left - 30, h - 73, 10, paint);
        }

        private void drawChip(Canvas canvas, float x, float y, String text, int color) {
            paint.setTextSize(11);
            paint.setTypeface(Typeface.create("sans", Typeface.BOLD));
            float width = paint.measureText(text) + 22;
            rect.set(x, y, x + width, y + 30);
            paint.setColor(0xAA071321);
            canvas.drawRoundRect(rect, 15, 15, paint);
            paint.setColor(color);
            canvas.drawText(text, x + 11, y + 20, paint);
        }

        private int withAlpha(int color, int alpha) {
            return (color & 0x00FFFFFF) | (alpha << 24);
        }

        @Override public boolean onTouchEvent(MotionEvent event) {
            if (event.getAction() == MotionEvent.ACTION_UP) {
                touches++;
                invalidate();
                Log.i("ScarletCompat", "SCARLET_APK_TOUCH " + run);
                performClick();
            }
            return true;
        }

        @Override public boolean performClick() {
            super.performClick();
            return true;
        }
    }

    @Override public void onResume() {
        super.onResume();
        Log.i("ScarletCompat", "SCARLET_APK_RESUMED " + run);
    }
}

package dev.scarlet.compat;

import android.app.Activity;
import android.os.Bundle;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.util.Log;
import android.view.MotionEvent;
import android.view.View;

public final class MainActivity extends Activity {
    private String run;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        run = getIntent().getStringExtra("scarlet_run");
        setContentView(new View(this) {
            private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
            private int touches;

            @Override protected void onDraw(Canvas canvas) {
                canvas.drawColor(Color.rgb(22, 34, 57));
                paint.setColor(Color.rgb(255, 126, 116));
                canvas.drawRect(32, 40, getWidth() - 32, 140, paint);
                paint.setColor(Color.WHITE);
                paint.setTextSize(30);
                canvas.drawText("Scarlet Android", 48, 103, paint);
                canvas.drawText("Touches: " + touches, 48, 210, paint);
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

            @Override public boolean performClick() { super.performClick(); return true; }
        });
    }

    @Override public void onResume() {
        super.onResume();
        Log.i("ScarletCompat", "SCARLET_APK_RESUMED " + run);
    }
}

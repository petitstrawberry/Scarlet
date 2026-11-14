package org.scarlet.android.input;

import android.hardware.input.InputManager;
import android.view.InputEvent;
import android.view.MotionEvent;

import java.io.IOException;
import java.lang.reflect.InvocationTargetException;

/** Injects stock Android mouse MotionEvents through the platform InputManager. */
final class AndroidMouseSink implements MouseState.Sink {
    private static final int INJECT_INPUT_EVENT_MODE_ASYNC = 0;
    private static final int SOURCE_MOUSE = 0x00002002;
    private static final int DISPLAY_ID = 0;
    private static final int DEVICE_ID = 0;
    private static final int FLAGS = 0;

    private final InputManager inputManager;

    AndroidMouseSink(InputManager inputManager) {
        this.inputManager = inputManager;
    }

    @Override
    public void emit(MouseState.Event event) throws IOException {
        MotionEvent.PointerProperties properties = new MotionEvent.PointerProperties();
        properties.id = 0;
        properties.toolType = MotionEvent.TOOL_TYPE_MOUSE;

        MotionEvent.PointerCoords coordinates = new MotionEvent.PointerCoords();
        coordinates.x = event.x;
        coordinates.y = event.y;
        coordinates.pressure = event.buttonState == 0 ? 0f : 1f;
        if (event.scrollAxis == MouseState.SCROLL_HORIZONTAL) {
            coordinates.setAxisValue(MotionEvent.AXIS_HSCROLL, event.scrollValue);
        } else if (event.scrollAxis == MouseState.SCROLL_VERTICAL) {
            coordinates.setAxisValue(MotionEvent.AXIS_VSCROLL, event.scrollValue);
        }

        MotionEvent motionEvent = MotionEvent.obtain(
                event.downTimeMs,
                event.eventTimeMs,
                event.action,
                1,
                new MotionEvent.PointerProperties[] {properties},
                new MotionEvent.PointerCoords[] {coordinates},
                event.metaState,
                event.buttonState,
                1f,
                1f,
                DEVICE_ID,
                0,
                SOURCE_MOUSE,
                FLAGS);
        try {
            setDisplayId(motionEvent, DISPLAY_ID);
            setActionButton(motionEvent, event.actionButton);
            boolean accepted;
            try {
                accepted = (Boolean) PlatformInputManager.INJECT_INPUT_EVENT.invoke(
                        inputManager, motionEvent,
                        INJECT_INPUT_EVENT_MODE_ASYNC);
            } catch (IllegalAccessException failure) {
                throw PlatformInputManager.reflectionFailure("accessing InputManager.injectInputEvent", failure);
            } catch (InvocationTargetException failure) {
                throw PlatformInputManager.reflectionFailure("injecting mouse event", failure.getCause());
            }
            if (!accepted) {
                throw new MouseState.RejectedEventException(event.action, event.x, event.y);
            }
        } finally {
            motionEvent.recycle();
        }
    }

    private static void setDisplayId(MotionEvent event, int displayId) throws IOException {
        try {
            PlatformInputManager.SET_DISPLAY_ID.invoke(event, displayId);
        } catch (IllegalAccessException failure) {
            throw PlatformInputManager.reflectionFailure("accessing MotionEvent.setDisplayId", failure);
        } catch (InvocationTargetException failure) {
            throw PlatformInputManager.reflectionFailure("setting MotionEvent display id", failure.getCause());
        }
    }

    private static void setActionButton(MotionEvent event, int actionButton) throws IOException {
        try {
            PlatformInputManager.SET_ACTION_BUTTON.invoke(event, actionButton);
        } catch (IllegalAccessException failure) {
            throw PlatformInputManager.reflectionFailure("accessing MotionEvent.setActionButton", failure);
        } catch (InvocationTargetException failure) {
            throw PlatformInputManager.reflectionFailure("setting MotionEvent action button", failure.getCause());
        }
    }
}


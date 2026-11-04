package org.scarlet.android.input;

/** Decoder for the fixed-size Android mouse transport frame. */
public final class MouseProtocol {
    public static final int FRAME_SIZE = 40;
    public static final int MAGIC = 0x314d4153;
    public static final int VERSION = 1;

    public static final int KIND_POSITION = 1;
    public static final int KIND_BUTTON = 2;
    public static final int KIND_SCROLL = 3;
    public static final int KIND_CANCEL = 4;

    public static final int BTN_LEFT = 272;
    public static final int BTN_RIGHT = 273;
    public static final int BTN_MIDDLE = 274;
    public static final int REL_HWHEEL = 6;
    public static final int REL_WHEEL = 8;

    static final int MAX_ABS_COORDINATE = 1_000_000;
    static final int MAX_SCROLL_STEPS = 1_000;
    static final int MODIFIER_MASK = 0x7f;

    private MouseProtocol() {}

    public static final class Frame {
        public final int kind;
        public final int x;
        public final int y;
        public final int code;
        public final int value;
        public final long timestampNs;
        public final int modifiers;

        Frame(int kind, int x, int y, int code, int value, long timestampNs, int modifiers) {
            this.kind = kind;
            this.x = x;
            this.y = y;
            this.code = code;
            this.value = value;
            this.timestampNs = timestampNs;
            this.modifiers = modifiers;
        }
    }

    public static Frame decode(byte[] bytes) throws ProtocolException {
        if (bytes.length != FRAME_SIZE) {
            throw new ProtocolException("frame must be exactly " + FRAME_SIZE + " bytes");
        }

        int magic = getInt(bytes, 0);
        int version = getUnsignedShort(bytes, 4);
        int kind = getUnsignedShort(bytes, 6);
        int x = getInt(bytes, 8);
        int y = getInt(bytes, 12);
        int code = getInt(bytes, 16);
        int value = getInt(bytes, 20);
        long timestampNs = getLong(bytes, 24);
        int modifiers = getInt(bytes, 32);
        int reserved = getInt(bytes, 36);

        if (magic != MAGIC) {
            throw new ProtocolException("bad magic");
        }
        if (version != VERSION) {
            throw new ProtocolException("unsupported version " + version);
        }
        if (kind < KIND_POSITION || kind > KIND_CANCEL) {
            throw new ProtocolException("unknown kind " + kind);
        }
        if (x < -MAX_ABS_COORDINATE || x > MAX_ABS_COORDINATE
                || y < -MAX_ABS_COORDINATE || y > MAX_ABS_COORDINATE) {
            throw new ProtocolException("coordinate out of range");
        }
        if (timestampNs < 0) {
            throw new ProtocolException("timestamp does not fit the supported u64 range");
        }
        if ((modifiers & ~MODIFIER_MASK) != 0) {
            throw new ProtocolException("modifier bits outside the supported mask");
        }
        if (reserved != 0) {
            throw new ProtocolException("reserved word is not zero");
        }

        switch (kind) {
            case KIND_POSITION:
            case KIND_CANCEL:
                if (code != 0 || value != 0) {
                    throw new ProtocolException("position/cancel fields must be zero");
                }
                break;
            case KIND_BUTTON:
                if (code != BTN_LEFT && code != BTN_RIGHT && code != BTN_MIDDLE) {
                    throw new ProtocolException("unsupported button code " + code);
                }
                if (value != 0 && value != 1) {
                    throw new ProtocolException("button value must be 0 or 1");
                }
                break;
            case KIND_SCROLL:
                if (code != REL_HWHEEL && code != REL_WHEEL) {
                    throw new ProtocolException("unsupported scroll code " + code);
                }
                if (value == 0 || value < -MAX_SCROLL_STEPS || value > MAX_SCROLL_STEPS) {
                    throw new ProtocolException("scroll value out of range");
                }
                break;
            default:
                throw new AssertionError(kind);
        }

        return new Frame(kind, x, y, code, value, timestampNs, modifiers);
    }

    private static int getUnsignedShort(byte[] bytes, int offset) {
        return (bytes[offset] & 0xff) | ((bytes[offset + 1] & 0xff) << 8);
    }

    private static int getInt(byte[] bytes, int offset) {
        return (bytes[offset] & 0xff)
                | ((bytes[offset + 1] & 0xff) << 8)
                | ((bytes[offset + 2] & 0xff) << 16)
                | ((bytes[offset + 3] & 0xff) << 24);
    }

    private static long getLong(byte[] bytes, int offset) {
        return (getInt(bytes, offset) & 0xffffffffL)
                | ((long) getInt(bytes, offset + 4) << 32);
    }

    public static final class ProtocolException extends Exception {
        ProtocolException(String message) {
            super(message);
        }
    }

}


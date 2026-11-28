package fixture;

/** An enum with a constructor, a field, a constant-specific body, and an interface. */
public enum Color implements Iface {
    RED(0xFF0000),
    GREEN(0x00FF00) {
        @Override
        public String describe() {
            return "green!";
        }
    },
    BLUE(0x0000FF);

    private final int rgb;

    Color(int rgb) {
        this.rgb = rgb;
    }

    public int rgb() {
        return rgb;
    }

    @Override
    public String describe() {
        return name().toLowerCase();
    }
}

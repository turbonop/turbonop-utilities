package fixture;

import java.util.List;

/** A record: components, a compact constructor, an annotated and a generic component. */
public record Point(int x, @Info(id = 9) int y, List<String> tags) implements Comparable<Point> {
    public static final Point ORIGIN = new Point(0, 0);

    public Point {
        if (x < 0) {
            throw new IllegalArgumentException("x");
        }
        tags = List.copyOf(tags);
    }

    public Point(int x, int y) {
        this(x, y, List.of());
    }

    @Override
    public int compareTo(Point o) {
        return Integer.compare(x * x + y * y, o.x * o.x + o.y * o.y);
    }
}

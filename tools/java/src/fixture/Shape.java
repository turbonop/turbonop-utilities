package fixture;

/** A sealed hierarchy: final, sealed, and non-sealed subclasses. */
public sealed interface Shape permits Shape.Circle, Shape.Square, Shape.Polygon {
    double area();

    record Circle(double r) implements Shape {
        public double area() {
            return Math.PI * r * r;
        }
    }

    final class Square implements Shape {
        final double side;

        Square(double side) {
            this.side = side;
        }

        public double area() {
            return side * side;
        }
    }

    sealed abstract class Polygon implements Shape permits Triangle, Other {}

    final class Triangle extends Polygon {
        public double area() {
            return 0.5;
        }
    }

    non-sealed class Other extends Polygon {
        public double area() {
            return 1;
        }
    }

    static String name(Shape s) {
        return switch (s) {
            case Circle c -> "circle " + c.r();
            case Square q -> "square " + q.side;
            case Triangle t -> "triangle";
            case Other o -> "other";
        };
    }
}

package fixture;

import java.io.Serializable;
import java.util.Collection;
import java.util.List;
import java.util.Map;
import java.util.function.BiFunction;

/** Generic signatures of every shape. */
public abstract class Generics<K extends Comparable<? super K>, V extends Number & Serializable, E extends Exception>
        implements Map.Entry<K, List<V>> {
    public K key;
    public List<? extends V>[] lists;
    public Map<? super K, ?> wild;
    public Generics<K, V, E>.Node<String> node;
    public int[][] prims;
    public List<int[]> listOfArrays;
    public V[] varr;

    public class Node<T> {
        public T value;
        public Generics<K, V, E> outer() {
            return Generics.this;
        }
    }

    public static class Pair<A, B> {
        public A a;
        public B b;
    }

    public abstract <T extends K> T pick(Collection<? extends T> from) throws E;

    public static <A, B extends A, C extends List<? super B>> C copy(A a, B b, C c) {
        c.add(b);
        return c;
    }

    public <R> R apply(BiFunction<? super K, ? super V, ? extends R> f, K k, V v) {
        List<? extends R> local = List.of(f.apply(k, v));
        return local.get(0);
    }

    public abstract void raw(List list, Map.Entry<String, Pair<int[], String[][]>> entry);
}

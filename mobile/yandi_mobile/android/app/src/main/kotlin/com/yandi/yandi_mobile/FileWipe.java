package com.yandi.yandi_mobile;

import java.io.File;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.security.SecureRandom;

/**
 * Затирание файла поверх его старых байтов и удаление. Файл НЕ обнуляется перед записью (иначе система освободила бы старые блоки, а новые
 * данные легли бы в другие): запись идёт в тех же местах. Честная оговорка: на флеш-памяти с выравниванием износа это лучшее усилие,
 * гарантия — шифрование файла собственным ключом (защищённое хранилище): тогда достаточно удалить ключ.
 */
public final class FileWipe {
    private FileWipe() {}

    public static boolean wipe(File f) {
        if (!f.exists()) return true;
        try (RandomAccessFile raf = new RandomAccessFile(f, "rw")) {
            long left = raf.length();
            byte[] buf = new byte[64 * 1024];
            SecureRandom rnd = new SecureRandom();
            int blocks = 0;
            raf.seek(0);
            while (left > 0) {
                if (blocks++ % 16 == 0) rnd.nextBytes(buf);   // свежие случайные байты раз в 1 МБ: быстро и без повторяющегося шаблона на диске
                int n = (int) Math.min(left, buf.length);
                raf.write(buf, 0, n);
                left -= n;
            }
            raf.getFD().sync();
        } catch (IOException ignored) {
            // не вышло затереть — всё равно удаляем
        }
        return f.delete();
    }
}

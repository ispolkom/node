import com.yandi.yandi_mobile.FileWipe;
import java.io.File;
import java.io.FileInputStream;
import java.nio.file.Files;
import java.util.Arrays;

public class FileWipeTest {
    public static void main(String[] args) throws Exception {
        File f = File.createTempFile("wipe", ".bin");
        byte[] secret = new byte[3 * 1024 * 1024 + 123];
        Arrays.fill(secret, (byte) 0x41);
        Files.write(f.toPath(), secret);
        // держим открытый дескриптор: на Linux он читаем и после удаления, значит видно, что осталось на месте старых байтов
        try (FileInputStream held = new FileInputStream(f)) {
            long before = f.length();
            if (!FileWipe.wipe(f)) throw new AssertionError("wipe() вернул false");
            if (f.exists()) throw new AssertionError("файл остался");
            byte[] left = held.readAllBytes();
            if (left.length != before) throw new AssertionError("файл был обнулён/обрезан: " + left.length + " вместо " + before);
            int same = 0;
            for (byte b : left) if (b == 0x41) same++;
            if (same > left.length / 100) throw new AssertionError("старое содержимое осталось: " + same + " байт из " + left.length);
        }
        if (!FileWipe.wipe(new File("/нет/такого/файла"))) throw new AssertionError("отсутствующий файл — не ошибка");
        System.out.println("FileWipe tests passed");
    }
}

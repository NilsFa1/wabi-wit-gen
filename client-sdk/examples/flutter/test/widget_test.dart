// `flutter test` on the host: the build hook bundles the host's core library,
// so the widget talks to the real native core here, too.
import 'package:wabi_flutter_example/main.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('shows a customer from the native core', (tester) async {
    await tester.pumpWidget(const WabiExampleApp());
    await tester.runAsync(() async {
      for (var i = 0; i < 100 && find.text('customer: Erika Mustermann').evaluate().isEmpty; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 50));
        await tester.pump();
      }
    });
    expect(find.text('customer: Erika Mustermann'), findsOneWidget);
    expect(find.byKey(const Key('core')), findsOneWidget);
  });
}
